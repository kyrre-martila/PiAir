//! Protocol composition over RustCrypto primitives. No AirPlay sender dependency.
use anyhow::{ensure, Result};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305,
};
use ed25519_dalek::{Signature, VerifyingKey};
use hkdf::Hkdf;
use sha2::{Digest, Sha512};
use zeroize::{Zeroize, Zeroizing};

pub fn hash(parts: &[&[u8]]) -> Zeroizing<[u8; 64]> {
    let mut h = Sha512::new();
    for part in parts {
        h.update(part);
    }
    Zeroizing::new(h.finalize().into())
}
pub fn derive(secret: &[u8], salt: &str, info: &str) -> Zeroizing<[u8; 32]> {
    let mut key = Zeroizing::new([0; 32]);
    Hkdf::<Sha512>::new(Some(salt.as_bytes()), secret)
        .expand(info.as_bytes(), key.as_mut())
        .expect("fixed valid HKDF size");
    key
}
pub fn seal(
    key: &[u8; 32],
    nonce: &[u8; 12],
    msg: &[u8],
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    ChaCha20Poly1305::new(key.into())
        .encrypt(nonce.into(), Payload { msg, aad })
        .map(Zeroizing::new)
        .map_err(|_| anyhow::anyhow!("encryption failed"))
}
pub fn open(
    key: &[u8; 32],
    nonce: &[u8; 12],
    msg: &[u8],
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    ChaCha20Poly1305::new(key.into())
        .decrypt(nonce.into(), Payload { msg, aad })
        .map(Zeroizing::new)
        .map_err(|_| anyhow::anyhow!("authentication tag invalid"))
}
pub fn nonce(label: &[u8; 8]) -> [u8; 12] {
    let mut n = [0; 12];
    n[4..].copy_from_slice(label);
    n
}
pub fn verify(key: &[u8; 32], msg: &[u8], sig: &[u8]) -> Result<()> {
    VerifyingKey::from_bytes(key)?
        .verify_strict(msg, &Signature::from_slice(sig)?)
        .map_err(|_| anyhow::anyhow!("receiver signature invalid"))
}
/// Directional keys owned only by the authenticated control transport.
pub struct ControlKeys {
    write: Zeroizing<[u8; 32]>,
    read: Zeroizing<[u8; 32]>,
    tx: u64,
    rx: u64,
    failed: bool,
}
impl ControlKeys {
    #[cfg(test)]
    pub(super) fn swap_directions_for_test(&mut self) {
        std::mem::swap(&mut self.read, &mut self.write);
    }

    pub fn new(shared: &[u8]) -> Self {
        Self {
            write: derive(shared, "Control-Salt", "Control-Write-Encryption-Key"),
            read: derive(shared, "Control-Salt", "Control-Read-Encryption-Key"),
            tx: 0,
            rx: 0,
            failed: false,
        }
    }
    fn invalidate(&mut self) {
        self.failed = true;
        self.read.zeroize();
        self.write.zeroize();
    }
    pub fn encrypt(&mut self, msg: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let result = self.encrypt_inner(msg);
        if result.is_err() {
            self.invalidate();
        }
        result
    }
    fn encrypt_inner(&mut self, msg: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        ensure!(
            !self.failed && !msg.is_empty() && msg.len() <= 1024 && self.tx < u64::MAX,
            "invalid control block or exhausted nonce"
        );
        let length = (msg.len() as u16).to_le_bytes();
        let encrypted = seal(&self.write, &nonce(&self.tx.to_le_bytes()), msg, &length)?;
        self.tx += 1;
        let mut out = Zeroizing::new(length.to_vec());
        out.extend_from_slice(&encrypted);
        Ok(out)
    }
    pub fn decrypt(&mut self, frame: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let result = self.decrypt_inner(frame);
        if result.is_err() {
            self.invalidate();
        }
        result
    }
    fn decrypt_inner(&mut self, frame: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        ensure!(
            !self.failed && frame.len() >= 18 && self.rx < u64::MAX,
            "invalid control frame"
        );
        let length = u16::from_le_bytes(frame[..2].try_into()?) as usize;
        ensure!(
            (1..=1024).contains(&length) && frame.len() == length + 18,
            "invalid control frame size"
        );
        let msg = open(
            &self.read,
            &nonce(&self.rx.to_le_bytes()),
            &frame[2..],
            &frame[..2],
        )?;
        self.rx += 1;
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<Vec<u8>> {
        include_str!("../../tests/fixtures/hap-control.hex")
            .lines()
            .map(|line| {
                let (pairs, remainder) = line.as_bytes().as_chunks::<2>();
                assert!(remainder.is_empty());
                pairs
                    .iter()
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect()
            })
            .collect()
    }
    #[test]
    fn independent_hap_control_fixture_both_directions_and_counters() {
        let fixtures = fixture();
        let secret: Vec<u8> = (0..32).collect();
        let mut client = ControlKeys::new(&secret);
        for (i, plain) in [
            b"OPTIONS * RTSP/1.0\r\nCSeq: 3\r\n\r\n".as_slice(),
            b"second block",
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(client.encrypt(plain).unwrap().as_slice(), fixtures[i]);
            assert_eq!(client.decrypt(&fixtures[i + 2]).unwrap().as_slice(), *plain);
        }
        // The same authenticated bytes are not valid a second time.
        assert!(client.decrypt(&fixtures[3]).is_err());
        assert!(client.write.iter().all(|b| *b == 0));
        assert!(client.read.iter().all(|b| *b == 0));
        assert!(client.encrypt(b"cannot reuse").is_err());
    }
    #[test]
    fn nonce_exhaustion_and_bad_length_permanently_invalidate_both_keys() {
        for receive in [false, true] {
            let mut keys = ControlKeys::new(b"secret");
            if receive {
                keys.rx = u64::MAX;
                assert!(keys.decrypt(&fixture()[2]).is_err());
            } else {
                keys.tx = u64::MAX;
                assert!(keys.encrypt(b"last block").is_err());
            }
            assert!(keys.failed);
            assert!(keys.encrypt(b"retry").is_err());
            assert!(keys.decrypt(&fixture()[2]).is_err());
        }
        let mut keys = ControlKeys::new(b"secret");
        assert_eq!(keys.encrypt(&[0; 1024]).unwrap().len(), 1042);
        assert!(keys.encrypt(b"").is_err());
        assert!(keys.encrypt(b"retry").is_err());
    }
}
