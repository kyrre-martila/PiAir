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
    pub fn response(&self, state: u8, allowed: &[u8]) -> Result<()> {
        ensure!(self.get(STATE)? == [state], "unexpected pairing state");
        if self.0.contains_key(&7) {
            bail!("receiver rejected pairing/authentication; no retry or fallback");
        }
        ensure!(
            self.0.keys().all(|k| allowed.contains(k) || *k == STATE),
            "unsupported pairing fields or method"
        );
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
