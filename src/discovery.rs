//! Passive DNS-SD only. No receiver transport, probing, or selection belongs here.
use crate::config::device_id;
use anyhow::{Context, Result};
use mdns_sd::{DaemonEvent, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    thread,
    time::{Duration, Instant},
};

const AIRPLAY: &str = "_airplay._tcp.local.";
const RAOP: &str = "_raop._tcp.local.";
const CAPABILITIES: &[&str] = &[
    "features",
    "ft",
    "model",
    "am",
    "srcvers",
    "osvers",
    "fv",
    "vv",
    "cn",
    "ch",
    "sr",
    "ss",
    "et",
    "tp",
    "sf",
    "flags",
    "pw",
    "pi",
    "protovers",
    "acl",
    "act",
    "gid",
    "gpn",
    "tsid",
    "gcgl",
    "igl",
];

#[derive(Clone, Debug)]
pub struct Advertisement {
    pub fullname: String,
    pub service_type: String,
    pub port: u16,
    pub addresses: BTreeSet<IpAddr>,
    pub capabilities: BTreeMap<String, String>,
    id: Option<String>,
    name: String,
}

#[derive(Debug)]
pub struct Receiver {
    pub id: String,
    pub name: String,
    pub services: Vec<Advertisement>,
}

/// Keep only bounded, printable UTF-8 values for recognized fields. Unknown TXT
/// (including public keys) is neither retained nor logged. DNS keys ignore case.
fn parse_txt<'a>(
    fields: impl IntoIterator<Item = (&'a str, Option<&'a [u8]>)>,
) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for (key, bytes) in fields {
        let key = key.to_ascii_lowercase();
        if key != "deviceid" && !CAPABILITIES.contains(&key.as_str()) {
            continue;
        }
        let value = bytes
            .and_then(|b| std::str::from_utf8(b).ok())
            .filter(|v| !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control));
        let Some(value) = value else {
            if ["gid", "gpn", "tsid", "gcgl", "igl", "cn", "ch", "sr", "ss"].contains(&key.as_str())
            {
                values.insert(key.clone(), "invalid".into());
            }
            if [
                "deviceid",
                "pi",
                "features",
                "ft",
                "flags",
                "sf",
                "pw",
                "acl",
                "act",
                "protovers",
            ]
            .contains(&key.as_str())
            {
                values.insert("deviceid".into(), "invalid".into());
            }
            continue;
        };
        if values.get(&key).is_some_and(|old| old != value) {
            values.insert("deviceid".into(), "conflicting".into());
            values.insert(key, "conflicting".into());
        } else {
            values.entry(key).or_insert_with(|| value.to_string());
        }
    }
    values
}

impl Advertisement {
    #[cfg(test)]
    pub(crate) fn test_from_info(info: &ServiceInfo) -> Self {
        Self::from_info(info).unwrap()
    }

    pub fn matches_id(&self, id: &str) -> bool {
        self.id.as_deref() == Some(id)
    }
    /// DNS-SD encodes low then optional high 32-bit hex words. No truncation.
    pub fn features(&self) -> Option<u64> {
        let parse = |value: &str| {
            let mut words = value.split(',');
            let word = |s: &str| {
                let s = s
                    .strip_prefix("0x")
                    .or_else(|| s.strip_prefix("0X"))
                    .unwrap_or(s);
                if s.is_empty() || s.len() > 8 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return None;
                }
                u32::from_str_radix(s, 16).ok()
            };
            let low = word(words.next()?)? as u64;
            let high = match words.next() {
                Some(s) => word(s)?,
                None => 0,
            };
            if words.next().is_some() {
                return None;
            }
            Some(low | (high as u64) << 32)
        };
        let features = match self.capabilities.get("features") {
            Some(v) => Some(parse(v)?),
            None => None,
        };
        let ft = match self.capabilities.get("ft") {
            Some(v) => Some(parse(v)?),
            None => None,
        };
        match (features, ft) {
            (Some(a), Some(b)) if a != b => None,
            (a, b) => a.or(b),
        }
    }

    fn from_info(info: &ServiceInfo) -> Option<Self> {
        let txt = parse_txt(info.get_properties().iter().map(|p| (p.key(), p.val())));
        Self::new(
            info.get_fullname(),
            info.get_type(),
            info.get_port(),
            info.get_addresses().iter().copied().collect(),
            txt,
        )
    }

    fn new(
        fullname: &str,
        service_type: &str,
        port: u16,
        addresses: BTreeSet<IpAddr>,
        mut txt: BTreeMap<String, String>,
    ) -> Option<Self> {
        let service_type = service_type.to_ascii_lowercase();
        if ![AIRPLAY, RAOP].contains(&service_type.as_str()) || port == 0 || addresses.is_empty() {
            return None;
        }
        let lower = fullname.to_ascii_lowercase();
        if !lower.ends_with(&format!(".{service_type}")) {
            return None;
        }
        let instance = &fullname[..fullname.len() - service_type.len() - 1];
        if instance.is_empty() || instance.chars().any(char::is_control) {
            return None;
        }
        let raop_parts = (service_type == RAOP)
            .then(|| instance.split_once('@'))
            .flatten();
        let instance_id = raop_parts.and_then(|(prefix, _)| device_id(prefix));
        let raw_id = txt.remove("deviceid");
        let malformed_id = raw_id.as_ref().is_some_and(|v| device_id(v).is_none());
        let txt_id = raw_id.and_then(|value| device_id(&value));
        // Conflicting advertised identities are not reliable enough to merge.
        let id = match (instance_id, txt_id) {
            _ if malformed_id => None,
            (Some(a), Some(b)) if a != b => None,
            (a, b) => a.or(b),
        };
        let name = raop_parts
            .map(|(_, name)| name)
            .filter(|name| !name.is_empty())
            .unwrap_or(instance);
        Some(Self {
            fullname: fullname.to_string(),
            service_type,
            port,
            addresses,
            capabilities: txt,
            id,
            name: name.to_string(),
        })
    }
}

#[derive(Default)]
struct Records(BTreeMap<String, Advertisement>);

impl Records {
    fn upsert(&mut self, advertisement: Advertisement) {
        self.0
            .insert(advertisement.fullname.to_ascii_lowercase(), advertisement);
    }

    fn remove(&mut self, fullname: &str) {
        self.0.remove(&fullname.to_ascii_lowercase());
    }

    fn receivers(self) -> Vec<Receiver> {
        let mut receivers: BTreeMap<String, Receiver> = BTreeMap::new();
        for (fullname, advertisement) in self.0 {
            // Fallback ID is service-scoped and cannot be allowlisted. Never merge
            // by display name, hostname, or IP address (which can be shared).
            let id = advertisement
                .id
                .clone()
                .unwrap_or_else(|| format!("service:{fullname}"));
            let receiver = receivers.entry(id.clone()).or_insert_with(|| Receiver {
                id,
                name: advertisement.name.clone(),
                services: Vec::new(),
            });
            receiver.services.push(advertisement);
        }
        receivers.into_values().collect()
    }
}

fn collect(daemon: &ServiceDaemon, timeout: Duration) -> Result<Vec<Receiver>> {
    let monitor = daemon.monitor().context("cannot monitor mDNS failures")?;
    let channels = [daemon.browse(AIRPLAY)?, daemon.browse(RAOP)?];
    let deadline = Instant::now() + timeout;
    let mut records = Records::default();
    tracing::debug!(
        timeout_secs = timeout.as_secs(),
        "passive mDNS discovery started"
    );
    while Instant::now() < deadline {
        // Limit each batch so continuous multicast traffic cannot extend timeout.
        for channel in &channels {
            for _ in 0..32 {
                let Ok(event) = channel.try_recv() else { break };
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        if let Some(advertisement) = Advertisement::from_info(&info) {
                            records.upsert(advertisement);
                        } else {
                            records.remove(info.get_fullname());
                            tracing::debug!("ignored malformed or incomplete advertisement");
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => records.remove(&fullname),
                    _ => {}
                }
            }
        }
        for _ in 0..32 {
            let Ok(event) = monitor.try_recv() else { break };
            if let DaemonEvent::Error(error) = event {
                return Err(error).context("mDNS failed; check multicast support and interfaces");
            }
        }
        thread::sleep(
            Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    let receivers = records.receivers();
    tracing::debug!(
        receivers = receivers.len(),
        "passive mDNS discovery finished"
    );
    Ok(receivers)
}

pub fn discover(timeout_secs: u64) -> Result<Vec<Receiver>> {
    let daemon =
        ServiceDaemon::new().context("cannot start mDNS; check UDP 5353 and multicast access")?;
    let result = collect(&daemon, Duration::from_secs(timeout_secs));
    // Always request shutdown, including after a partially successful browse setup.
    let shutdown = (|| -> Result<()> {
        for service in [AIRPLAY, RAOP] {
            if daemon.stop_browse(service).is_err() {
                tracing::debug!(
                    service,
                    "could not stop browse separately; shutting down daemon"
                );
            }
        }
        daemon
            .shutdown()?
            .recv_timeout(Duration::from_secs(1))
            .context("mDNS shutdown acknowledgment timed out")?;
        Ok(())
    })();
    match (result, shutdown) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error).context("mDNS cleanup failed"),
        (Ok(receivers), Ok(())) => Ok(receivers),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ad(instance: &str, service: &str, id: Option<&str>) -> Advertisement {
        let txt = id
            .map(|id| ("deviceid".to_string(), id.to_string()))
            .into_iter()
            .collect();
        Advertisement::new(
            &format!("{instance}.{service}"),
            service,
            7000,
            ["192.0.2.1".parse().unwrap()].into(),
            txt,
        )
        .unwrap()
    }

    #[test]
    fn txt_handles_binary_missing_and_unknown_values() {
        let txt = parse_txt([
            ("FEATURES", Some(&b"0x1,0x2"[..])),
            ("pw", None),
            ("model", Some(&b"\xff"[..])),
            ("sr", Some(&b"44\n100"[..])),
            ("pk", Some(&b"never retained"[..])),
            ("fv", Some(&b"27.2"[..])),
        ]);
        assert_eq!(txt.len(), 4);
        assert_eq!(txt["sr"], "invalid");
        assert_eq!(txt["fv"], "27.2");
        assert!(device_id(&txt["deviceid"]).is_none());
        assert_eq!(txt["features"], "0x1,0x2");
    }

    #[test]
    fn matching_ids_merge_but_endpoints_remain_separate() {
        let mut records = Records::default();
        records.upsert(ad("Room", AIRPLAY, Some("AA:BB:CC:DD:EE:FF")));
        records.upsert(ad("AABBCCDDEEFF@Room", RAOP, None));
        records.upsert(ad("Room", AIRPLAY, Some("AA:BB:CC:DD:EE:FF")));
        let receivers = records.receivers();
        assert_eq!(receivers.len(), 1);
        assert_eq!(receivers[0].id, "device:aabbccddeeff");
        assert_eq!(receivers[0].services.len(), 2);
    }

    #[test]
    fn names_and_ips_do_not_merge_unknown_or_conflicting_ids() {
        let mut records = Records::default();
        records.upsert(ad("Room", AIRPLAY, None));
        records.upsert(ad("Room", RAOP, None));
        records.upsert(ad("AABBCCDDEEFF@Room", RAOP, Some("11:22:33:44:55:66")));
        let receivers = records.receivers();
        assert_eq!(receivers.len(), 3);
        assert!(receivers.iter().all(|r| r.id.starts_with("service:")));
    }

    #[test]
    fn updates_replace_stale_endpoints_and_removals_work() {
        let mut records = Records::default();
        records.upsert(ad("Room", AIRPLAY, Some("aabbccddeeff")));
        let mut updated = ad("Room", AIRPLAY, Some("aabbccddeeff"));
        updated.port = 7001;
        records.upsert(updated);
        assert_eq!(records.0.len(), 1);
        assert_eq!(records.0.values().next().unwrap().port, 7001);
        records.remove("ROOM._AIRPLAY._TCP.LOCAL.");
        assert!(records.receivers().is_empty());
    }

    #[test]
    fn malformed_advertisements_and_ids() {
        let ips = ["192.0.2.1".parse().unwrap()].into();
        assert!(
            Advertisement::new("._airplay._tcp.local.", AIRPLAY, 1, ips, BTreeMap::new()).is_none()
        );
        assert!(Advertisement::new(
            "Room._http._tcp.local.",
            "_http._tcp.local.",
            80,
            BTreeSet::new(),
            BTreeMap::new()
        )
        .is_none());
        for id in [
            "",
            "bad",
            "a:b:c:d:e:f",
            "AA-BB-CC-DD-EE-FF",
            "000000000000",
            "ffffffffffff",
        ] {
            assert!(device_id(id).is_none());
        }
        assert!(ad("Room", AIRPLAY, Some("bad")).id.is_none());
        let mut advertisement = ad("Room", AIRPLAY, Some("aabbccddeeff"));
        advertisement.port = 0;
        assert!(Advertisement::new(
            &advertisement.fullname,
            AIRPLAY,
            0,
            advertisement.addresses,
            BTreeMap::new()
        )
        .is_none());
    }
    #[test]
    fn feature_words_and_conflicting_txt_fail_closed() {
        let mut advertisement = ad("Room", AIRPLAY, Some("aabbccddeeff"));
        for (text, expected) in [
            ("0x0,0x4000", Some(1u64 << 46)),
            ("0x1", Some(1)),
            ("0x100000000", None),
            ("0x1,0x2,0x3", None),
            ("0x1,", None),
            ("bad-value", None),
        ] {
            advertisement
                .capabilities
                .insert("features".into(), text.into());
            assert_eq!(advertisement.features(), expected);
        }
        let txt = parse_txt([
            ("deviceid", Some(&b"aabbccddeeff"[..])),
            ("DEVICEID", Some(&b"112233445566"[..])),
        ]);
        let conflict = Advertisement::new(
            "Room._airplay._tcp.local.",
            AIRPLAY,
            7000,
            ["127.0.0.1".parse().unwrap()].into(),
            txt,
        )
        .unwrap();
        assert!(conflict.id.is_none());
        assert!(ad("AABBCCDDEEFF@Room", RAOP, Some("malformed"))
            .id
            .is_none());
    }
    #[test]
    fn invalid_security_txt_poisons_identity_instead_of_disappearing() {
        for key in [
            "pw",
            "flags",
            "sf",
            "acl",
            "act",
            "protovers",
            "pi",
            "features",
            "ft",
        ] {
            let txt = parse_txt([
                ("deviceid", Some(&b"aabbccddeeff"[..])),
                (key, Some(&b"\xff"[..])),
            ]);
            let ad = Advertisement::new(
                "Room._airplay._tcp.local.",
                AIRPLAY,
                7000,
                ["127.0.0.1".parse().unwrap()].into(),
                txt,
            )
            .unwrap();
            assert!(ad.id.is_none(), "{key}");
        }
    }
}
