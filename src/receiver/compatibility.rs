//! Local policy over untrusted advertisements, not receiver compatibility proof.
//! Evidence and deliberate unsupported paths: docs/PAIRING.md.
use crate::discovery::Advertisement;
use anyhow::{bail, ensure, Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Method {
    /// Persistent HAP method 0, with an explicit AirPlay PIN-display request.
    PinDisplay,
    /// Persistent HAP method 0 with a user-supplied existing setup code; no PIN-start.
    SetupCode,
    /// Authentication-only persistent HAP, established by validated v2 credentials.
    /// Never an enrollment choice or a persisted selection value.
    #[value(skip)]
    #[serde(skip)]
    StoredHap,
    /// Connection-scoped SRP with no persistent identity verification.
    Transient,
}
impl Method {
    pub fn pin_start(self) -> bool {
        self == Self::PinDisplay
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    #[serde(with = "super::hex_u64")]
    pub features: u64,
    pub flags: Option<u32>,
    pub password: Option<bool>,
    pub access_control: Option<u64>,
    #[serde(default)]
    pub access_control_type: Option<u64>,
    pub protocol_version: Option<String>,
}
impl Capabilities {
    /// Only operational status may drift after selection. Keep feature words,
    /// authentication/access policy, protocol version, unknown bits and even
    /// presence/absence pinned. See docs/PAIRING.md for evidence and assumptions.
    pub fn matches_selection(&self, selected: &Self) -> bool {
        const OPERATIONAL: u32 = (1 << 2) // audio cable attached
            | (1 << 11) // relay/current-track availability
            | (1 << 12) // silent primary
            | (1 << 13) // synchronization group leader
            | (1 << 14) // synchronization buddy unreachable
            | (1 << 17) // receiver session active
            | (1 << 20); // observed Apple TV local TV playback status
        let flags_match = match (self.flags, selected.flags) {
            (Some(current), Some(saved)) => (current ^ saved) & !OPERATIONAL == 0,
            (None, None) => true,
            _ => false,
        };
        flags_match
            && self.features == selected.features
            && self.password == selected.password
            && self.access_control == selected.access_control
            && self.access_control_type == selected.access_control_type
            && self.protocol_version == selected.protocol_version
    }
    pub fn parse(service: &Advertisement) -> Result<Self> {
        let txt = &service.capabilities;
        let flags = txt
            .get("flags")
            .or_else(|| txt.get("sf"))
            .map(|s| hex_word(s))
            .transpose()?;
        if let (Some(a), Some(b)) = (txt.get("flags"), txt.get("sf")) {
            ensure!(
                hex_word(a)? == hex_word(b)?,
                "contradictory flags/sf advertisements; inspect receiver configuration"
            );
        }
        let password = txt
            .get("pw")
            .map(|v| match v.as_str() {
                "true" => Ok(true),
                "false" => Ok(false),
                _ => bail!("malformed password capability; expected pw=true or false"),
            })
            .transpose()?;
        let access_control = txt
            .get("acl")
            .map(|v| {
                ensure!(
                    !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()),
                    "malformed receiver access-control capability"
                );
                v.parse::<u64>()
                    .context("invalid receiver access-control value")
            })
            .transpose()?;
        Ok(Self {
            features: service
                .features()
                .context("missing, conflicting or malformed AirPlay feature words")?,
            flags,
            password,
            access_control,
            access_control_type: txt
                .get("act")
                .map(|v| v.parse::<u64>().context("malformed access-control type"))
                .transpose()?,
            protocol_version: txt.get("protovers").cloned(),
        })
    }
    /// Never infer a method from name/model, never substitute another method.
    /// Advertised unknown non-pairing bits are retained, not assigned semantics.
    pub fn decide(&self, requested: Method) -> Result<Method> {
        ensure!(
            !(self.password == Some(false) && self.flags.is_some_and(|f| f & (1 << 7) != 0)),
            "contradictory password capabilities; inspect receiver configuration"
        );
        ensure!(self.password!=Some(true) && self.flags.is_none_or(|f|f&(1<<7)==0),"password-protected AirPlay authentication is unsupported; a HAP setup PIN is not an AirPlay password");
        ensure!(self.access_control.is_none_or(|acl|acl==0),"nonzero receiver access-control policy is unresolved; Home membership authorization is not implemented");
        ensure!(
            self.protocol_version
                .as_deref()
                .is_none_or(|v| ["1.0", "1.1"].contains(&v)),
            "unknown AirPlay protocol version; collect documented firmware evidence before use"
        );
        ensure!(
            self.access_control_type.is_none_or(|act| act == 0),
            "receiver access-control type unsupported; no Home authorization or fallback"
        );
        // Bit 51 advertises a capability, not mandatory MFi. Pinned OwnTone's
        // persistent HAP path does not require auth-setup/MFi (see source analysis).
        // Explicit method 0 still requires bit 46 and rejects MFi TLVs on wire.
        if requested == Method::Transient {
            if self.features & ((1 << 43) | (1 << 48)) == 0 {
                bail!("receiver does not advertise system/transient pairing (bits 43/48)");
            }
            let flags = self
                .flags
                .context("transient authentication requires advertised status flags")?;
            ensure!(flags & ((1 << 3) | (1 << 9)) == 0,
                "receiver requires PIN or persistent pairing; transient profile unsupported; no fallback");
            return Ok(requested);
        }
        ensure!(self.features & (1<<46)!=0,"persistent HAP pairing is not established by these capabilities; bit 38/43/48 alone is insufficient, and legacy/FairPlay paths are unsupported; no fallback");
        Ok(requested)
    }
    pub fn buffered_audio_advertised(&self) -> bool {
        self.features & (1 << 40) != 0
    }
}
fn hex_word(text: &str) -> Result<u32> {
    let value = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    ensure!(
        !value.is_empty() && value.len() <= 8 && value.bytes().all(|b| b.is_ascii_hexdigit()),
        "malformed receiver status flags"
    );
    Ok(u32::from_str_radix(value, 16)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn caps(features: u64) -> Capabilities {
        Capabilities {
            features,
            flags: None,
            password: None,
            access_control: None,
            access_control_type: None,
            protocol_version: None,
        }
    }
    #[test]
    fn stua_reported_flags_keep_numeric_provenance_and_persistent_policy() {
        // Decimal diagnostic output and literal TXT hex have different values.
        // Decimal 99908 has bit 9 set; literal TXT hex 99908 has bit 3 set.
        for (flags, required_bit) in [(99908, 9), (hex_word("99908").unwrap(), 3)] {
            assert_ne!(flags & (1 << required_bit), 0);
            let mut c = caps((1 << 46) | (1 << 48) | (1 << 40) | (1 << 41));
            c.flags = Some(flags);
            assert!(c.decide(Method::StoredHap).is_ok());
            assert!(c.decide(Method::Transient).is_err());
            let mut changed = c.clone();
            changed.flags = Some(flags ^ (1 << 9));
            assert!(!changed.matches_selection(&c));
            assert!(changed.decide(Method::StoredHap).is_ok());
            c.access_control = Some(1);
            assert!(c.decide(Method::StoredHap).is_err());
        }
        assert_eq!(99908, 0x18644);
        assert_eq!(hex_word("99908").unwrap(), 0x99908);
    }

    #[test]
    fn only_explicit_operational_status_bits_may_change() {
        let mut saved = caps(1 << 46);
        saved.flags = Some(0x18644);
        for bit in 0..32 {
            let mut observed = saved.clone();
            observed.flags = saved.flags.map(|f| f ^ (1 << bit));
            assert_eq!(
                observed.matches_selection(&saved),
                [2, 11, 12, 13, 14, 17, 20].contains(&bit),
                "bit {bit}"
            );
            assert_eq!(
                saved.matches_selection(&observed),
                [2, 11, 12, 13, 14, 17, 20].contains(&bit),
                "bit {bit}"
            );
        }
        let mut observed = saved.clone();
        observed.flags = None;
        assert!(!observed.matches_selection(&saved));
        for change in 0..4 {
            let mut observed = saved.clone();
            match change {
                0 => observed.features ^= 1 << 40,
                1 => observed.password = Some(false),
                2 => observed.access_control = Some(0),
                _ => observed.protocol_version = Some("1.1".into()),
            }
            assert!(!observed.matches_selection(&saved));
        }
    }
    #[test]
    fn decisions_do_not_confuse_pairing_features_with_playback_or_fallback() {
        for features in [
            1 << 46,
            (1 << 46) | (1 << 38),
            (1 << 46) | (1 << 43) | (1 << 48),
            (1 << 46) | (1 << 40) | (1 << 41),
            (1 << 46) | (1 << 51),
        ] {
            for method in [Method::PinDisplay, Method::SetupCode] {
                assert_eq!(caps(features).decide(method).unwrap(), method);
            }
        }
        for features in [
            0,
            1 << 38,
            1 << 43,
            1 << 48,
            (1 << 40) | (1 << 41),
            1 << 27,
            1 << 51,
        ] {
            for method in [Method::PinDisplay, Method::SetupCode, Method::Transient] {
                assert!(caps(features).decide(method).is_err());
            }
        }
        assert!(caps((1 << 46) | (1 << 48))
            .decide(Method::Transient)
            .is_err());
        assert!(!caps(1 << 46).buffered_audio_advertised());
    }
    #[test]
    fn contradictory_password_unknown_version_and_access_control_fail_closed() {
        let mut c = caps(1 << 46);
        c.password = Some(false);
        c.flags = Some(1 << 7);
        assert!(c
            .decide(Method::PinDisplay)
            .unwrap_err()
            .to_string()
            .contains("contradictory"));
        c.password = Some(true);
        assert!(c.decide(Method::SetupCode).is_err());
        c.password = None;
        c.flags = None;
        c.protocol_version = Some("2.0".into());
        assert!(c.decide(Method::PinDisplay).is_err());
        c.protocol_version = Some("1.1".into());
        c.access_control = Some(2);
        assert!(c.decide(Method::PinDisplay).is_err());
    }
}

#[cfg(test)]
mod transient_tests {
    use super::*;
    #[test]
    fn explicit_transient_requires_capability_and_unprotected_status() {
        let base = Capabilities {
            features: 1 << 48,
            flags: Some(0x98404),
            password: None,
            access_control: None,
            access_control_type: None,
            protocol_version: Some("1.1".into()),
        };
        assert_eq!(base.decide(Method::Transient).unwrap(), Method::Transient);
        assert!(base.decide(Method::PinDisplay).is_err());
        for bit in [3, 7, 9] {
            let mut c = base.clone();
            c.flags = Some(0x98404 | (1 << bit));
            assert!(c.decide(Method::Transient).is_err());
        }
        for change in 0..6 {
            let mut c = base.clone();
            match change {
                0 => c.features = 1 << 46,
                1 => c.flags = None,
                2 => c.password = Some(true),
                3 => c.access_control = Some(1),
                4 => c.access_control_type = Some(2),
                _ => c.protocol_version = Some("unknown".into()),
            }
            assert!(c.decide(Method::Transient).is_err());
        }
    }
}
