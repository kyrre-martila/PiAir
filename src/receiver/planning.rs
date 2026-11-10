//! Passive, sans-I/O evidence and policy. Never authorizes a socket or changes selection.
use super::compatibility::{Capabilities, Method};
use crate::discovery::{Advertisement, Receiver};
use plist::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Claim<T> {
    Unknown,
    Invalid,
    Conflict,
    Observed(T),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provenance {
    Mdns { service: String },
    AuthenticatedInfo,
}
/// Safe protocol scalars only. Never retain entire /info, keys, or credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    pub provenance: Provenance,
    pub raw: BTreeMap<String, String>,
    pub features: Claim<u64>,
    pub status_flags: Claim<u64>,
    pub realtime_format: Claim<u64>,
    pub buffered_formats: Claim<BTreeSet<u64>>,
}
impl Evidence {
    pub fn mdns(ad: &Advertisement) -> Self {
        let raw = ad
            .capabilities
            .iter()
            .filter(|(k, _)| {
                [
                    "features",
                    "ft",
                    "flags",
                    "sf",
                    "pw",
                    "acl",
                    "act",
                    "protovers",
                    "cn",
                    "sr",
                    "ss",
                    "ch",
                    "gid",
                    "gpn",
                    "tsid",
                    "gcgl",
                    "igl",
                ]
                .contains(&k.as_str())
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Self {
            provenance: Provenance::Mdns {
                service: ad.fullname.clone(),
            },
            raw,
            features: match ad.features() {
                Some(f) => Claim::Observed(f),
                None if ad.capabilities.contains_key("features")
                    || ad.capabilities.contains_key("ft") =>
                {
                    Claim::Invalid
                }
                None => Claim::Unknown,
            },
            status_flags: Claim::Unknown,
            realtime_format: Claim::Unknown,
            buffered_formats: Claim::Unknown,
        }
    }
    /// Caller must have authenticated the same receiver on this connection.
    /// This pure parser does not establish authentication or correlate identities.
    #[allow(dead_code)] // Future authenticated-session seam; passive CLI never fetches /info.
    pub fn authenticated_info(value: &Value) -> Self {
        let integer = |path: &[&str]| {
            let mut v = value;
            for key in path {
                let Some(d) = v.as_dictionary() else {
                    return Claim::Invalid;
                };
                let Some(next) = d.get(key) else {
                    return Claim::Unknown;
                };
                v = next;
            }
            v.as_unsigned_integer()
                .map(Claim::Observed)
                .unwrap_or(Claim::Invalid)
        };
        let mask = integer(&["supportedFormats", "bufferStream"]);
        let extended = value
            .as_dictionary()
            .and_then(|d| d.get("supportedAudioFormatsExtended"))
            .and_then(Value::as_dictionary)
            .and_then(|d| d.get("bufferStream"));
        let list = match extended {
            None if value
                .as_dictionary()
                .and_then(|d| d.get("supportedAudioFormatsExtended"))
                .is_some_and(|v| v.as_dictionary().is_none()) =>
            {
                Claim::Invalid
            }
            None if value.as_dictionary().is_none() => Claim::Invalid,
            None => Claim::Unknown,
            Some(v) => match v.as_array() {
                Some(a) if a.len() <= 64 => a
                    .iter()
                    .map(Value::as_unsigned_integer)
                    .collect::<Option<BTreeSet<_>>>()
                    .map(Claim::Observed)
                    .unwrap_or(Claim::Invalid),
                _ => Claim::Invalid,
            },
        };
        let raw_mask = mask.clone();
        let raw_list = list.clone();
        let mask = match mask {
            Claim::Observed(m) => {
                Claim::Observed((0..64).filter(|bit| m & (1u64 << bit) != 0).collect())
            }
            Claim::Unknown => Claim::Unknown,
            _ => Claim::Invalid,
        };
        let buffered_formats = reconcile(mask, list);
        let features = integer(&["features"]);
        let realtime_format = integer(&["supportedFormats", "audioStream"]);
        let status_flags = integer(&["statusFlags"]);
        let mut raw = BTreeMap::new();
        // Retain only typed scalars. Sensitive and unknown payloads are discarded.
        for (key, claim) in [
            ("features", &features),
            ("statusFlags", &status_flags),
            ("supportedFormats.bufferStream", &raw_mask),
            ("supportedFormats.audioStream", &realtime_format),
        ] {
            if let Claim::Observed(n) = claim {
                raw.insert(key.into(), format!("0x{n:x}"));
            }
        }
        if let Claim::Observed(ref formats) = raw_list {
            raw.insert(
                "supportedAudioFormatsExtended.bufferStream".into(),
                format!("{formats:?}"),
            );
        }
        if let Claim::Observed(ref formats) = buffered_formats {
            raw.insert("bufferStream.formatIDs".into(), format!("{formats:?}"));
        }
        Self {
            provenance: Provenance::AuthenticatedInfo,
            raw,
            features,
            status_flags,
            realtime_format,
            buffered_formats,
        }
    }
}
fn reconcile<T: PartialEq>(a: Claim<T>, b: Claim<T>) -> Claim<T> {
    match (a, b) {
        (Claim::Invalid, _) | (_, Claim::Invalid) => Claim::Invalid,
        (Claim::Unknown, b) => b,
        (a, Claim::Unknown) => a,
        (Claim::Observed(a), Claim::Observed(b)) if a == b => Claim::Observed(a),
        _ => Claim::Conflict,
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Raop,
    RealtimeNtp,
    RealtimePtp,
    BufferedPtp,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Implementation {
    DiagnosticOnly,
    Unsupported,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub transport: Transport,
    pub implementation: Implementation,
    pub rejections: Vec<String>,
}
/// No runtime result is inferred from discovery, /info or successful RTSP.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RuntimeResults {
    pub negotiated: Option<Transport>,
    pub media_sent: Option<bool>,
    pub audible_playback: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiverCapabilities {
    pub evidence: Vec<Evidence>,
    pub raop: Claim<bool>,
    pub realtime: Claim<bool>,
    pub buffered: Claim<bool>,
    pub ptp: Claim<bool>,
    pub ntp: Claim<bool>,
    pub alac: Claim<bool>,
    pub sample_format: Claim<(u32, u16, u16)>,
    pub authentication: Vec<Claim<Capabilities>>,
    pub runtime: RuntimeResults,
}
impl ReceiverCapabilities {
    pub fn observe(receiver: &Receiver) -> Self {
        let evidence: Vec<_> = receiver.services.iter().map(Evidence::mdns).collect();
        let features = evidence
            .iter()
            .fold(Claim::Unknown, |a, e| reconcile(a, e.features.clone()));
        let bit = |n: u32| match features {
            Claim::Observed(f) => Claim::Observed(f & (1u64 << n) != 0u64),
            ref other => match other {
                Claim::Invalid => Claim::Invalid,
                Claim::Conflict => Claim::Conflict,
                _ => Claim::Unknown,
            },
        };
        let sample_format = receiver.services.iter().fold(Claim::Unknown, |a, s| {
            let parse = || {
                Some((
                    s.capabilities.get("sr")?.parse().ok()?,
                    s.capabilities.get("ss")?.parse().ok()?,
                    s.capabilities.get("ch")?.parse().ok()?,
                ))
            };
            let b = if ["sr", "ss", "ch"]
                .iter()
                .all(|k| !s.capabilities.contains_key(*k))
            {
                Claim::Unknown
            } else {
                parse().map(Claim::Observed).unwrap_or(Claim::Invalid)
            };
            reconcile(a, b)
        });
        Self {
            raop: if receiver
                .services
                .iter()
                .any(|s| s.service_type == "_raop._tcp.local.")
            {
                Claim::Observed(true)
            } else {
                Claim::Unknown
            },
            // Audio is a prerequisite claim, not proof of acceptance of type 96.
            realtime: bit(9),
            buffered: bit(40),
            ptp: bit(41),
            ntp: Claim::Unknown,
            alac: bit(19),
            sample_format,
            authentication: receiver
                .services
                .iter()
                .filter(|s| s.service_type == "_airplay._tcp.local.")
                .map(|s| {
                    Capabilities::parse(s)
                        .map(Claim::Observed)
                        .unwrap_or(Claim::Invalid)
                })
                .collect(),
            evidence,
            runtime: RuntimeResults::default(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopologyClaim {
    pub receiver: String,
    pub service: String,
    pub group: Claim<String>,
    pub stereo_pair: Claim<String>,
    pub leader: Claim<bool>,
    pub contains_leader: Claim<bool>,
}
fn group(value: Option<&String>) -> Claim<String> {
    match value {
        None => Claim::Unknown,
        Some(s) => {
            let base = s
                .strip_suffix("+0")
                .or_else(|| s.strip_suffix("+1"))
                .unwrap_or(s);
            if uuid::Uuid::parse_str(base).is_ok() {
                Claim::Observed(s.to_ascii_lowercase())
            } else {
                Claim::Invalid
            }
        }
    }
}
fn flag(value: Option<&String>) -> Claim<bool> {
    match value.map(String::as_str) {
        None => Claim::Unknown,
        Some("1" | "true") => Claim::Observed(true),
        Some("0" | "false") => Claim::Observed(false),
        _ => Claim::Invalid,
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct Topology {
    pub claims: Vec<TopologyClaim>,
    /// Shared explicit gid or tsid only; never confirmed membership or elected leaders.
    pub candidate_sets: BTreeMap<String, BTreeSet<String>>,
    pub ambiguous: BTreeSet<String>,
    /// Explicit igl hints with peers sharing gid; no product-class inference.
    pub proxy_candidates: BTreeSet<String>,
    /// Shared tsid claims, never confirmed stereo membership.
    pub stereo_candidates: BTreeMap<String, BTreeSet<String>>,
}
/// Rebuild from the current live discovery snapshot. No carry-over of expired claims.
pub fn topology(receivers: &[Receiver]) -> Topology {
    let mut topology = Topology {
        claims: Vec::new(),
        candidate_sets: BTreeMap::new(),
        ambiguous: BTreeSet::new(),
        proxy_candidates: BTreeSet::new(),
        stereo_candidates: BTreeMap::new(),
    };
    for r in receivers {
        let mut groups = BTreeSet::new();
        let mut pairs = BTreeSet::new();
        for s in &r.services {
            let c = TopologyClaim {
                receiver: r.id.clone(),
                service: s.fullname.clone(),
                group: group(s.capabilities.get("gid")),
                stereo_pair: group(s.capabilities.get("tsid")),
                leader: flag(s.capabilities.get("igl")),
                contains_leader: flag(s.capabilities.get("gcgl")),
            };
            for (kind, claim, distinct) in [
                ("gid", &c.group, &mut groups),
                ("tsid", &c.stereo_pair, &mut pairs),
            ] {
                if let Claim::Observed(id) = claim {
                    distinct.insert(id.clone());
                    topology
                        .candidate_sets
                        .entry(format!("{kind}:{id}"))
                        .or_default()
                        .insert(r.id.clone());
                } else if *claim == Claim::Invalid {
                    topology.ambiguous.insert(r.id.clone());
                }
            }
            if c.leader == Claim::Invalid || c.contains_leader == Claim::Invalid {
                topology.ambiguous.insert(r.id.clone());
            }
            topology.claims.push(c);
        }
        if groups.len() > 1 || pairs.len() > 1 {
            topology.ambiguous.insert(r.id.clone());
        }
    }
    for (key, members) in &topology.candidate_sets {
        if members.len() < 2 {
            continue;
        }
        if key.starts_with("tsid:") {
            topology
                .stereo_candidates
                .insert(key.clone(), members.clone());
        } else {
            topology.proxy_candidates.extend(
                topology
                    .claims
                    .iter()
                    .filter(|c| members.contains(&c.receiver) && c.leader == Claim::Observed(true))
                    .map(|c| c.receiver.clone()),
            );
        }
    }
    // Multiple explicit leader hints are ambiguous, not an election opportunity.
    for members in topology.candidate_sets.values() {
        if topology
            .claims
            .iter()
            .filter(|c| members.contains(&c.receiver) && c.leader == Claim::Observed(true))
            .map(|c| &c.receiver)
            .collect::<BTreeSet<_>>()
            .len()
            > 1
        {
            topology.ambiguous.extend(members.iter().cloned());
        }
    }
    topology
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Authorization {
    pub allowlisted: bool,
    pub method: Option<Method>,
    /// Explicit future authenticated timing restriction; PTP advertisement alone is not this.
    pub requires_ptp: bool,
}
/// Stable preference order; eligible candidates first. Returned values are advice,
/// never executable sessions. Revalidate authorization/selection at the existing wire boundary.
pub fn plan(
    receiver: &Receiver,
    authorization: Authorization,
    topology: &Topology,
    info: Option<&Evidence>,
) -> Vec<Candidate> {
    let mut caps = ReceiverCapabilities::observe(receiver);
    if let Some(info) = info {
        caps.evidence.push(info.clone());
        let feature = caps
            .evidence
            .iter()
            .fold(Claim::Unknown, |a, e| reconcile(a, e.features.clone()));
        caps.realtime = match feature {
            Claim::Observed(f) => Claim::Observed(f & (1 << 9) != 0),
            Claim::Unknown => Claim::Unknown,
            Claim::Invalid => Claim::Invalid,
            Claim::Conflict => Claim::Conflict,
        };
    }
    let mut candidates = Vec::new();
    for transport in [
        Transport::RealtimeNtp,
        Transport::RealtimePtp,
        Transport::BufferedPtp,
        Transport::Raop,
    ] {
        let mut c = Candidate {
            transport,
            implementation: if transport == Transport::RealtimeNtp {
                Implementation::DiagnosticOnly
            } else {
                Implementation::Unsupported
            },
            rejections: Vec::new(),
        };
        if !authorization.allowlisted {
            c.rejections
                .push("receiver is not authorized by allowlist".into());
        }
        if c.implementation == Implementation::Unsupported {
            c.rejections
                .push("transport is not implemented for streaming".into());
        }
        if topology.ambiguous.contains(&receiver.id) {
            c.rejections.push("ambiguous topology claims".into());
        }
        if topology.claims.iter().any(|c| {
            c.receiver == receiver.id
                && (c.group != Claim::Unknown || c.stereo_pair != Claim::Unknown)
        }) {
            c.rejections.push(
                "group handling is unimplemented; candidate membership needs validation".into(),
            );
        }
        if let Some(info) = info {
            if info.provenance != Provenance::AuthenticatedInfo {
                c.rejections
                    .push("info evidence lacks authenticated provenance".into());
            }
            if matches!(info.status_flags, Claim::Invalid | Claim::Conflict)
                || matches!(info.status_flags, Claim::Observed(f) if f & (1 << 7) != 0)
            {
                c.rejections.push(
                    "authenticated receiver status requires unsupported authentication".into(),
                );
            }
            if authorization.method == Some(Method::Transient)
                && matches!(info.status_flags, Claim::Observed(f) if f & ((1 << 3) | (1 << 9)) != 0)
            {
                c.rejections.push("authenticated status requires PIN or persistent pairing; no transient downgrade".into());
            }
            if transport == Transport::BufferedPtp
                && matches!(info.buffered_formats, Claim::Invalid | Claim::Conflict)
            {
                c.rejections
                    .push("authenticated buffered format claims malformed or contradictory".into());
            }
            if transport == Transport::RealtimeNtp {
                match info.realtime_format {
                    Claim::Observed(f) if f & 0x40000 == 0 => c
                        .rejections
                        .push("authenticated info excludes realtime ALAC format".into()),
                    Claim::Invalid | Claim::Conflict => c
                        .rejections
                        .push("authenticated sample formats malformed".into()),
                    _ => (),
                }
            }
        }
        if transport == Transport::Raop {
            if caps.raop != Claim::Observed(true) {
                c.rejections.push("RAOP advertisement missing".into());
            }
        } else {
            if caps.realtime != Claim::Observed(true) || caps.alac != Claim::Observed(true) {
                c.rejections
                    .push("audio/ALAC claims missing, malformed or contradictory".into());
            }
            if transport == Transport::BufferedPtp && caps.buffered != Claim::Observed(true) {
                c.rejections
                    .push("buffered audio claim not established".into());
            }
            if matches!(transport, Transport::RealtimePtp | Transport::BufferedPtp)
                && caps.ptp != Claim::Observed(true)
            {
                c.rejections.push("PTP claim not established".into());
            }
            if transport == Transport::RealtimeNtp && authorization.requires_ptp {
                c.rejections
                    .push("receiver requires unsupported PTP; no timing fallback".into());
            }
            let services: Vec<_> = receiver
                .services
                .iter()
                .filter(|s| s.service_type == "_airplay._tcp.local.")
                .collect();
            if services.len() != 1 || !services[0].matches_id(&receiver.id) {
                c.rejections
                    .push("missing or ambiguous stable AirPlay endpoint identity".into());
            }
            match authorization.method {
                None => c.rejections.push("no explicit authentication method authorized; no automatic pairing or downgrade".into()),
                Some(method) => for auth in &caps.authentication {
                    match auth {
                        Claim::Observed(auth) => if let Err(e) = auth.decide(method) { c.rejections.push(e.to_string()); },
                        _ => c.rejections.push("authentication requirements malformed or unknown".into()),
                    }
                },
            }
            if let Claim::Observed(format) = caps.sample_format {
                if format != (44100, 16, 2) {
                    c.rejections.push(
                        "advertised sample format excludes local ALAC/44100/16/2 profile".into(),
                    );
                }
            } else if matches!(caps.sample_format, Claim::Invalid | Claim::Conflict) {
                c.rejections
                    .push("sample format claims malformed or contradictory".into());
            }
        }
        candidates.push(c);
    }
    candidates.sort_by_key(|c| !c.rejections.is_empty());
    candidates
}

pub fn diagnostic(
    receivers: &[Receiver],
    allows: impl Fn(&str) -> bool,
    method: Option<Method>,
) -> String {
    let mut text = String::from("Passive mDNS evidence only; no /info fetched, connections, selection or state writes.\nEligible means a diagnostic candidate, not verified playback; NTP acceptance unknown until explicit session.\n");
    let topology = topology(receivers);
    writeln!(
        text,
        "Topology (untrusted candidates; proxy/stereo roles unverified): {topology:?}"
    )
    .unwrap();
    for receiver in receivers {
        let caps = ReceiverCapabilities::observe(receiver);
        writeln!(text, "{} capabilities={caps:?}", receiver.id).unwrap();
        writeln!(text, "  authenticated /info: unknown (not fetched)").unwrap();
        for c in plan(
            receiver,
            Authorization {
                allowlisted: allows(&receiver.id),
                method,
                requires_ptp: false,
            },
            &topology,
            None,
        ) {
            writeln!(
                text,
                "  {:?}: {:?}; eligible={}; rejection reasons={:?}",
                c.transport,
                c.implementation,
                c.rejections.is_empty(),
                c.rejections
            )
            .unwrap();
        }
    }
    text
}

#[cfg(test)]
mod tests;
