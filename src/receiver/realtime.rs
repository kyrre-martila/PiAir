//! OwnTone-profile RTP/ALAC + NTP facts. Original implementation; no networking.
use super::{
    crypto,
    profile::{self, RATE, SPF},
};
use crate::audio::alac;
use anyhow::{ensure, Context, Result};
use rand::{rngs::OsRng, RngCore};
use std::{
    collections::VecDeque,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zeroize::{Zeroize, Zeroizing};

pub struct Clock {
    mono: Instant,
    ntp: u64,
}
impl Clock {
    pub fn new() -> Result<Self> {
        let wall = SystemTime::now().duration_since(UNIX_EPOCH)?;
        let ntp = ((wall.as_secs().wrapping_add(2_208_988_800) & 0xffff_ffff) << 32)
            | (((u64::from(wall.subsec_nanos())) << 32) / 1_000_000_000);
        Ok(Self {
            mono: Instant::now(),
            ntp,
        })
    }
    pub fn now(&self) -> u64 {
        self.instant(Instant::now())
    }
    pub fn instant(&self, instant: Instant) -> u64 {
        self.at(instant.saturating_duration_since(self.mono))
    }
    pub fn at(&self, elapsed: Duration) -> u64 {
        self.ntp
            .wrapping_add(elapsed.as_secs().wrapping_shl(32))
            .wrapping_add((u64::from(elapsed.subsec_nanos()) << 32) / 1_000_000_000)
    }
}
pub fn sample_time(samples: u64) -> Duration {
    Duration::new(
        samples / u64::from(RATE),
        ((samples % u64::from(RATE)) * 1_000_000_000 / u64::from(RATE)) as u32,
    )
}
pub fn timing_reply(request: &[u8], recv: u64, send: u64) -> Result<[u8; 32]> {
    ensure!(
        request.len() == 32 && request[..2] == [0x80, 0xd2],
        "invalid NTP timing query"
    );
    let mut out = [0; 32];
    out[..2].copy_from_slice(&[0x80, 0xd3]);
    out[2..4].copy_from_slice(&request[2..4]);
    out[8..16].copy_from_slice(&request[24..32]);
    out[16..24].copy_from_slice(&recv.to_be_bytes());
    out[24..].copy_from_slice(&send.to_be_bytes());
    Ok(out)
}
pub fn sync(first: bool, next: u32, latency: u32, ntp: u64) -> [u8; 20] {
    let mut out = [0; 20];
    out[0] = if first { 0x90 } else { 0x80 };
    out[1] = 0xd4;
    out[3] = 7;
    out[4..8].copy_from_slice(&next.wrapping_sub(latency).to_be_bytes());
    out[8..16].copy_from_slice(&ntp.to_be_bytes());
    out[16..].copy_from_slice(&next.to_be_bytes());
    out
}
pub struct Encoder {
    key: Zeroizing<[u8; 32]>,
    nonce: u64,
    pub sequence: u16,
    pub timestamp: u32,
    ssrc: u32,
    first: bool,
    failed: bool,
    cache: VecDeque<(u16, Vec<u8>, u8)>,
}
impl Encoder {
    pub fn new() -> Result<Self> {
        let mut key = Zeroizing::new([0; 32]);
        OsRng.try_fill_bytes(key.as_mut())?;
        Ok(Self {
            key,
            nonce: 0,
            sequence: rand::random(),
            timestamp: rand::random(),
            ssrc: rand::random(),
            first: true,
            failed: false,
            cache: VecDeque::new(),
        })
    }
    pub(super) fn key(&self) -> &[u8; 32] {
        &self.key
    }
    pub fn packet(&mut self, pcm: &[[i16; 2]]) -> Result<Zeroizing<Vec<u8>>> {
        let result = (|| {
            ensure!(
                !self.failed && self.nonce < u64::MAX,
                "media nonce exhausted or failed key"
            );
            ensure!(
                pcm.len() == SPF,
                "realtime ALAC packet must have 352 stereo frames"
            );
            let payload = Zeroizing::new(alac::encode(pcm)?);
            let mut header = [0; 12];
            header[0] = 0x80;
            header[1] = if self.first { 0xe0 } else { 0x60 };
            header[2..4].copy_from_slice(&self.sequence.to_be_bytes());
            header[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
            header[8..].copy_from_slice(&self.ssrc.to_be_bytes());
            let nonce = self.nonce.to_le_bytes();
            let sealed = crypto::seal(&self.key, &crypto::nonce(&nonce), &payload, &header[4..])?;
            let mut out = Zeroizing::new(header.to_vec());
            out.extend_from_slice(&sealed);
            out.extend_from_slice(&nonce);
            if self.cache.len() == 256 {
                self.cache.pop_front();
            }
            self.cache.push_back((self.sequence, out.to_vec(), 0));
            self.sequence = self.sequence.wrapping_add(1);
            self.timestamp = self.timestamp.wrapping_add(SPF as u32);
            self.nonce += 1;
            self.first = false;
            Ok(out)
        })();
        if result.is_err() {
            self.key.zeroize();
            self.failed = true;
        }
        result
    }
    pub fn resend(&mut self, seq: u16) -> Result<Option<&[u8]>> {
        if let Some((_, p, retries)) = self.cache.iter_mut().find(|(s, _, _)| *s == seq) {
            ensure!(*retries < 3, "retransmission retry limit exceeded");
            *retries += 1;
            return Ok(Some(p));
        }
        Ok(None)
    }
    pub fn flush(&mut self) {
        self.cache.clear();
        self.first = true;
    }
}
pub fn latency(headers: &std::collections::BTreeMap<String, String>) -> Result<u32> {
    let n = match headers.get("audio-latency") {
        Some(n) => {
            ensure!(
                !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()),
                "invalid latency"
            );
            n.parse::<u32>().context("invalid latency")?
        }
        None => profile::MIN_LATENCY,
    };
    ensure!(
        (profile::MIN_LATENCY..=profile::MAX_LATENCY).contains(&n),
        "receiver latency out of negotiated range"
    );
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_encoder() -> Encoder {
        let mut e = Encoder::new().unwrap();
        e.key.copy_from_slice(&(0..32).collect::<Vec<_>>());
        e.sequence = u16::MAX;
        e.timestamp = u32::MAX - 10;
        e.ssrc = 0x01020304;
        e
    }
    #[test]
    fn independent_media_fixture_and_wrap_preserve_nonce_uniqueness() {
        let mut e = fixture_encoder();
        let pcm = [[0; 2]; SPF];
        let first = e.packet(&pcm).unwrap();
        assert_eq!(
            first.as_slice(),
            include_bytes!("../../tests/fixtures/realtime-packet.bin")
        );
        assert_eq!(e.sequence, 0);
        assert_eq!(e.timestamp, 341);
        let second = e.packet(&pcm).unwrap();
        assert_eq!(&second[..4], &[0x80, 0x60, 0, 0]);
        assert_eq!(&second[second.len() - 8..], &1_u64.to_le_bytes());
        e.flush();
        let third = e.packet(&pcm).unwrap();
        assert_eq!(third[1], 0xe0);
        assert_eq!(&third[third.len() - 8..], &2_u64.to_le_bytes());
    }
    #[test]
    fn cache_retries_and_key_lifecycle_are_bounded() {
        let mut e = fixture_encoder();
        let packet = e.packet(&[[0; 2]; SPF]).unwrap();
        for _ in 0..3 {
            assert_eq!(e.resend(u16::MAX).unwrap().unwrap(), packet.as_slice());
        }
        assert!(e.resend(u16::MAX).is_err());
        for _ in 0..256 {
            e.packet(&[[0; 2]; SPF]).unwrap();
        }
        assert!(e.resend(u16::MAX).unwrap().is_none());
        assert_eq!(e.cache.len(), 256);
        e.nonce = u64::MAX;
        assert!(e.packet(&[[0; 2]; SPF]).is_err());
        assert!(e.key.iter().all(|b| *b == 0));
        assert!(e.packet(&[[0; 2]; SPF]).is_err());
        assert_ne!(
            Encoder::new().unwrap().key.as_ref(),
            Encoder::new().unwrap().key.as_ref()
        );
    }
    #[test]
    fn timing_and_sync_use_protocol_fields_and_monotonic_sample_time() {
        let mut q = [0; 32];
        q[..4].copy_from_slice(&[0x80, 0xd2, 0, 7]);
        q[24..].copy_from_slice(&0x0102030405060708_u64.to_be_bytes());
        let r = timing_reply(&q, 9, 10).unwrap();
        assert_eq!(&r[..8], &[0x80, 0xd3, 0, 7, 0, 0, 0, 0]);
        assert_eq!(&r[8..16], &q[24..]);
        assert_eq!(&r[16..24], &9_u64.to_be_bytes());
        assert_eq!(&r[24..], &10_u64.to_be_bytes());
        assert!(timing_reply(&q[..31], 9, 10).is_err());
        q[1] = 0xd3;
        assert!(timing_reply(&q, 9, 10).is_err());
        let s = sync(true, 10, 20, 123);
        assert_eq!(&s[..4], &[0x90, 0xd4, 0, 7]);
        assert_eq!(&s[4..8], &10_u32.wrapping_sub(20).to_be_bytes());
        assert_eq!(&s[8..16], &123_u64.to_be_bytes());
        assert_eq!(&s[16..], &10_u32.to_be_bytes());
        assert_eq!(sample_time(u64::from(RATE)), Duration::from_secs(1));
        assert_eq!(
            sample_time(u64::from(RATE) * 3600),
            Duration::from_secs(3600)
        );
        let clock = Clock {
            mono: Instant::now(),
            ntp: 0xffffffff00000000,
        };
        assert_eq!(clock.at(Duration::from_secs(1)), 0);
    }
}
