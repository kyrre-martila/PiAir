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
/// Future control transport keys. No caller can mistake these for an audio session.
#[allow(dead_code)] // Reserved control framing API; no application/streaming requests yet.
pub struct ControlKeys {
    write: Zeroizing<[u8; 32]>,
    read: Zeroizing<[u8; 32]>,
    tx: u64,
    rx: u64,
    failed: bool,
}
#[allow(dead_code)]
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
