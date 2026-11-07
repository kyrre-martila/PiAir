//! HAP's SHA-512 SRP-6a profile. The srp crate's high-level proof is NOT HAP
//! compatible; use its arithmetic API with HAP's padded u, hashed S and proofs.
use super::crypto::hash;
use anyhow::{ensure, Result};
use num_bigint::BigUint;
use rand::{rngs::OsRng, RngCore};
use sha2::Sha512;
use srp::{client::SrpClient, groups::G_3072};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

pub fn pad(bytes: &[u8]) -> Result<[u8; 384]> {
    ensure!(bytes.len() <= 384, "invalid SRP integer size");
    let mut out = [0; 384];
    out[384 - bytes.len()..].copy_from_slice(bytes);
    Ok(out)
}
pub fn proofs(
    a: &[u8],
    b: &[u8],
    salt: &[u8],
    k: &[u8],
) -> (Zeroizing<[u8; 64]>, Zeroizing<[u8; 64]>) {
    let mut xor = hash(&[&G_3072.n.to_bytes_be()]);
    for (x, y) in xor.iter_mut().zip(hash(&[&G_3072.g.to_bytes_be()]).iter()) {
        *x ^= y;
    }
    let m1 = hash(&[xor.as_ref(), hash(&[b"Pair-Setup"]).as_ref(), salt, a, b, k]);
    let m2 = hash(&[a, m1.as_ref(), k]);
    (m1, m2)
}
pub struct Client {
    pub public: [u8; 384],
    pub proof: Zeroizing<[u8; 64]>,
    expected: Zeroizing<[u8; 64]>,
    key: Zeroizing<[u8; 64]>,
}
impl Client {
    pub fn start(pin: &[u8], salt: &[u8], b: &[u8]) -> Result<Self> {
        let mut private = Zeroizing::new([0; 32]);
        OsRng.try_fill_bytes(private.as_mut())?;
        Self::with_private(b"Pair-Setup", pin, salt, b, &private[..])
    }
    fn with_private(
        username: &[u8],
        pin: &[u8],
        salt: &[u8],
        b: &[u8],
        private: &[u8],
    ) -> Result<Self> {
        ensure!(salt.len() == 16 && b.len() == 384, "invalid SRP parameters");
        let c = SrpClient::<Sha512>::new(&G_3072);
        let public = pad(&c.compute_public_ephemeral(private))?;
        let b_int = BigUint::from_bytes_be(b);
        ensure!(
            b_int != BigUint::default() && b_int < G_3072.n,
            "invalid SRP public key"
        );
        let u = BigUint::from_bytes_be(hash(&[&public, b]).as_ref());
        ensure!(u != BigUint::default(), "invalid SRP scrambling parameter");
        let multiplier = BigUint::from_bytes_be(
            hash(&[&G_3072.n.to_bytes_be(), &pad(&G_3072.g.to_bytes_be())?]).as_ref(),
        );
        let identity =
            Zeroizing::new(SrpClient::<Sha512>::compute_identity_hash(username, pin).to_vec());
        let x = SrpClient::<Sha512>::compute_x(&identity, salt);
        let secret = Zeroizing::new(
            c.compute_premaster_secret(
                &b_int,
                &multiplier,
                &x,
                &BigUint::from_bytes_be(private),
                &u,
            )
            .to_bytes_be(),
        );
        let key = hash(&[&secret]);
        let (proof, expected) = proofs(&public, b, salt, key.as_ref());
        Ok(Self {
            public,
            proof,
            expected,
            key,
        })
    }
    pub fn finish(self, proof: &[u8]) -> Result<Zeroizing<[u8; 64]>> {
        ensure!(
            bool::from(self.expected.as_ref().ct_eq(proof)),
            "SRP receiver proof invalid (incorrect PIN or receiver)"
        );
        Ok(self.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn apple_hap_r2_srp_vector() {
        let fixture: std::collections::BTreeMap<String, String> =
            toml::from_str(include_str!("../../tests/fixtures/hap-srp.toml")).unwrap();
        let decode = |name: &str| {
            let text = &fixture[name];
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
                .collect::<Vec<_>>()
        };
        let client = Client::with_private(
            b"alice",
            b"password123",
            &decode("salt"),
            &decode("b_public"),
            &decode("a"),
        )
        .unwrap();
        assert_eq!(client.public.as_slice(), decode("a_public"));
        assert_eq!(client.key.as_ref(), decode("key"));
    }
    #[test]
    fn malformed_and_zero_srp_keys_fail() {
        for b in [vec![], vec![0; 384], G_3072.n.to_bytes_be(), vec![255; 385]] {
            assert!(Client::start(b"1234", &[0; 16], &b).is_err());
        }
    }
}
