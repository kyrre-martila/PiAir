//! Bounded TLV8 for one pairing message (not lists or transport fragments).
use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

pub const IDENTIFIER: u8 = 1;
pub const SALT: u8 = 2;
pub const PUBLIC_KEY: u8 = 3;
pub const PROOF: u8 = 4;
pub const ENCRYPTED: u8 = 5;
pub const STATE: u8 = 6;
pub const SIGNATURE: u8 = 10;
pub const FLAGS: u8 = 19;
pub const LIMIT: usize = 8192;
pub struct Tlv(BTreeMap<u8, Zeroizing<Vec<u8>>>);
impl Tlv {
    pub fn parse(mut bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= LIMIT, "pairing message too large");
        let mut fields: BTreeMap<u8, Zeroizing<Vec<u8>>> = BTreeMap::new();
        let mut previous = None;
        while !bytes.is_empty() {
            ensure!(bytes.len() >= 2, "truncated TLV header");
            let (kind, len) = (bytes[0], bytes[1] as usize);
            bytes = &bytes[2..];
            ensure!(bytes.len() >= len && len > 0, "truncated or empty TLV");
            ensure!(
                kind != 12 && kind != 13 && kind != 255,
                "unsupported TLV fragmentation/list"
            );
            if fields.contains_key(&kind) {
                ensure!(previous == Some((kind, 255)), "ambiguous duplicate TLV");
            }
            fields
                .entry(kind)
                .or_default()
                .extend_from_slice(&bytes[..len]);
            previous = Some((kind, len));
            bytes = &bytes[len..];
        }
        Ok(Self(fields))
    }
    pub fn get(&self, kind: u8) -> Result<&[u8]> {
        self.0
            .get(&kind)
            .map(|v| v.as_slice())
            .context("required pairing field absent")
    }
    pub fn array<const N: usize>(&self, kind: u8) -> Result<[u8; N]> {
        self.get(kind)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid pairing field length"))
    }
    pub fn integer(&self, kind: u8) -> Result<Option<u32>> {
        let Some(bytes) = self.0.get(&kind) else {
            return Ok(None);
        };
        ensure!(
            (1..=4).contains(&bytes.len()),
            "invalid pairing integer length"
        );
        let mut padded = [0; 4];
        padded[..bytes.len()].copy_from_slice(bytes);
        Ok(Some(u32::from_le_bytes(padded)))
    }
    pub fn only(&self, allowed: &[u8]) -> Result<()> {
        // Report type numbers only. Values may contain credentials or secrets.
        let unexpected: Vec<u8> = self
            .0
            .keys()
            .copied()
            .filter(|kind| !allowed.contains(kind))
            .collect();
        ensure!(
            unexpected.is_empty(),
            "unexpected pairing TLV types: {unexpected:?}"
        );
        Ok(())
    }
    pub fn response(&self, state: u8, allowed: &[u8]) -> Result<()> {
        ensure!(self.get(STATE)? == [state], "unexpected pairing state");
        if let Some(error) = self.0.get(&7) {
            ensure!(
                error.len() == 1 && error[0] != 0,
                "malformed pairing error code"
            );
            let reason = match error[0] {
                1 => "unspecified error",
                2 => "incorrect setup code, signature or proof",
                3 => "backoff requested; do not retry automatically",
                4 => "pairing table full; inspect receiver-side records",
                5 => "maximum attempts reached; inspect receiver policy",
                6 => "method unavailable or already paired",
                7 => "receiver busy",
                _ => "unknown receiver authentication error",
            };
            bail!("receiver rejected pairing/authentication: {reason}; no retry or fallback");
        }
        let mut allowed = allowed.to_vec();
        allowed.push(STATE);
        self.only(&allowed)?;
        Ok(())
    }
}
pub fn encode(fields: &[(u8, &[u8])]) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(Vec::new());
    for (kind, bytes) in fields {
        for chunk in bytes.chunks(255) {
            out.extend_from_slice(&[*kind, chunk.len() as u8]);
            out.extend_from_slice(chunk);
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allowlist_diagnostics_contain_only_sorted_unexpected_type_numbers() {
        let marker = b"synthetic-secret-marker";
        let encoded = encode(&[
            (IDENTIFIER, marker),
            (PUBLIC_KEY, marker),
            (SIGNATURE, marker),
            (19, marker),
            (0, marker),
        ]);
        let payload = Tlv::parse(&encoded).unwrap();
        let diagnostic = format!(
            "{:#}",
            payload
                .only(&[IDENTIFIER, PUBLIC_KEY, SIGNATURE])
                .unwrap_err()
        );
        // Boolean assertion avoids printing either diagnostic or secret fixture data.
        assert!(diagnostic == "unexpected pairing TLV types: [0, 19]");
        let expected = encode(&[
            (IDENTIFIER, marker),
            (PUBLIC_KEY, marker),
            (SIGNATURE, marker),
        ]);
        assert!(Tlv::parse(&expected)
            .unwrap()
            .only(&[IDENTIFIER, PUBLIC_KEY, SIGNATURE])
            .is_ok());
    }

    #[test]
    fn outer_response_uses_the_same_safe_allowlist_diagnostic() {
        let encoded = encode(&[
            (STATE, &[6]),
            (ENCRYPTED, b"synthetic-secret-marker"),
            (19, b"private"),
        ]);
        let diagnostic = format!(
            "{:#}",
            Tlv::parse(&encoded)
                .unwrap()
                .response(6, &[ENCRYPTED])
                .unwrap_err()
        );
        assert!(diagnostic == "unexpected pairing TLV types: [19]");
    }

    #[test]
    fn fragmented_and_malformed() {
        let value = [42; 384];
        let encoded = encode(&[(PUBLIC_KEY, &value), (STATE, &[2])]);
        assert_eq!(
            Tlv::parse(&encoded).unwrap().get(PUBLIC_KEY).unwrap(),
            value
        );
        for bad in [
            &[6][..],
            &[6, 2, 1],
            &[6, 1, 2, 6, 1, 2],
            &[255, 0],
            &[3, 0],
        ] {
            assert!(Tlv::parse(bad).is_err());
        }
        assert!(Tlv::parse(&encoded[..encoded.len() - 1]).is_err());
    }
}
