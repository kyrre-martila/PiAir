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
    /// Recognized but unsupported: AirPlay transient wire profiles remain unresolved.
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
    pub features: u64,
    pub flags: Option<u32>,
    pub password: Option<bool>,
    pub access_control: Option<u64>,
    pub protocol_version: Option<String>,
}
impl Capabilities {
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
        ensure!(self.password!=Some(true) && !self.flags.is_some_and(|f|f&(1<<7)!=0),"password-protected AirPlay authentication is unsupported; a HAP setup PIN is not an AirPlay password");
        ensure!(self.access_control.is_none_or(|acl|acl==0),"nonzero receiver access-control policy is unresolved; Home membership authorization is not implemented");
        ensure!(
            self.protocol_version
                .as_deref()
                .is_none_or(|v| ["1.0", "1.1"].contains(&v)),
            "unknown AirPlay protocol version; collect documented firmware evidence before use"
        );
        // Bit 51 advertises a capability, not mandatory MFi. Pinned OwnTone's
        // persistent HAP path does not require auth-setup/MFi (see source analysis).
        // Explicit method 0 still requires bit 46 and rejects MFi TLVs on wire.
        if requested == Method::Transient {
            if self.features & ((1 << 43) | (1 << 48)) == 0 {
                bail!("receiver does not advertise system/transient pairing (bits 43/48)");
            }
            bail!("transient AirPlay pairing is not implemented: HKP/header/probe variants lack a validated wire profile; no persistent identity or credentials may be inferred; no fallback");
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
            protocol_version: None,
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
