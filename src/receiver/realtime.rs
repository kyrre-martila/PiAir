//! Shared realtime RTP/ALAC and NTP/PTP synchronization formats. No networking.
use super::{
    crypto,
    profile::{self, RATE, SPF},
};
use crate::audio::alac;
use anyhow::{ensure, Result};
use rand::{rngs::OsRng, RngCore};
use std::{collections::VecDeque, time::Duration};
use zeroize::{Zeroize, Zeroizing};

// Sender policy, independent of the receiver's informational RECORD header and
// of the stream offer's upper bound. OwnTone: output_buffer = start_buffer -
// AIRPLAY_AUDIO_LATENCY_MS. PiAir keeps its existing two-second total lead.
pub const PLAYBACK_LEAD_SAMPLES: u32 = 2 * RATE;
pub const STREAM_LATENCY_SAMPLES: u32 = profile::MIN_LATENCY;
pub const SYNC_OFFSET_SAMPLES: u32 = PLAYBACK_LEAD_SAMPLES - STREAM_LATENCY_SAMPLES;

pub fn sample_time(samples: u64) -> Duration {
    Duration::new(
        samples / u64::from(RATE),
        ((samples % u64::from(RATE)) * 1_000_000_000 / u64::from(RATE)) as u32,
    )
}
/// PTP RTCP time announcement (PT 215), from the same clock as UDP319/320.
/// The scheduler supplies the RTP position corresponding to `time` and its next
/// packet position. This serializer does not schedule or send audio.
pub fn ptp_sync(
    first: bool,
    current_rtp: u32,
    next_rtp: u32,
    clock_id: u64,
    time: super::ptp::time::Timestamp,
) -> Result<[u8; 28]> {
    ensure!(
        clock_id != 0 && clock_id != u64::MAX,
        "invalid PTP sync clock"
    );
    super::ptp::time::Timestamp::new(time.seconds, time.nanos)?;
    let nanos = u64::try_from(time.ns())?;
    let mut out = [0; 28];
    out[..4].copy_from_slice(&[if first { 0x90 } else { 0x80 }, 0xd7, 0, 6]);
    out[4..8].copy_from_slice(&current_rtp.to_be_bytes());
    out[8..16].copy_from_slice(&nanos.to_be_bytes());
    out[16..20].copy_from_slice(&next_rtp.wrapping_sub(STREAM_LATENCY_SAMPLES).to_be_bytes());
    out[20..].copy_from_slice(&clock_id.to_be_bytes());
    Ok(out)
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
    pub(super) fn from_verified_key(key: Zeroizing<[u8; 32]>) -> Result<Self> {
        let mut encoder = Self::new()?;
        encoder.key = key;
        // Retain the initial sequence policy used by the short diagnostic.
        // Longer WAV streams may wrap RTP sequence numbers; the independent
        // u64 nonce keeps increasing and must never be reset at that wrap.
        encoder.sequence %= u16::MAX - 375;
        encoder.nonce = u64::from(encoder.sequence);
        Ok(encoder)
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
        for (_, packet, _) in &mut self.cache {
            packet.zeroize();
        }
        self.cache.clear();
        self.first = true;
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        self.flush();
        self.key.zeroize();
    }
}
/// RECORD Audio-Latency is diagnostic only, as in OwnTone's response handler.
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
    fn bounded_wavs_through_49_seconds_round_trip_through_sequence_wrap() {
        use std::collections::HashSet;
        for (seconds, expected_packets) in [(10, 1504), (30, 4010), (49, 6390)] {
            let wav = super::super::wav::Playback::open(
                std::path::Path::new("tests/assets/audio/Pachelbel_Canon_audio_test.wav"),
                seconds,
            )
            .unwrap();
            assert_eq!(wav.frames.len(), seconds as usize * RATE as usize);
            let mut encoder = Encoder::from_verified_key(Zeroizing::new([42; 32])).unwrap();
            // Force wrap inside all durations, including the old 376 packet bound.
            encoder.sequence = u16::MAX - 400;
            encoder.nonce = u64::from(encoder.sequence);
            let origin = encoder.timestamp;
            let first_nonce = encoder.nonce;
            let total = wav.frames.len() + PLAYBACK_LEAD_SAMPLES as usize;
            let mut nonces = HashSet::new();
            let mut packets = 0;
            for sent in (0..total).step_by(SPF) {
                let pcm: Vec<_> = (sent..sent + SPF)
                    .map(|n| wav.frames.get(n).copied().unwrap_or([0; 2]))
                    .collect();
                let packet = encoder.packet(&pcm).unwrap();
                let nonce: [u8; 8] = packet[packet.len() - 8..].try_into().unwrap();
                assert!(nonces.insert(nonce));
                assert_eq!(u64::from_le_bytes(nonce), first_nonce + packets);
                assert_eq!(
                    u16::from_be_bytes(packet[2..4].try_into().unwrap()),
                    (u16::MAX - 400).wrapping_add(packets as u16)
                );
                assert_eq!(
                    u32::from_be_bytes(packet[4..8].try_into().unwrap()),
                    origin.wrapping_add(sent as u32)
                );
                assert_eq!(packet.len(), 1452); // 12 RTP + 1416 ALAC + 16 tag + 8 nonce
                let payload = crypto::open(
                    &[42; 32],
                    &crypto::nonce(&nonce),
                    &packet[12..packet.len() - 8],
                    &packet[4..12],
                )
                .unwrap();
                assert_eq!(alac::tests::decode_at_rate(&payload, RATE), pcm);
                packets += 1;
            }
            assert_eq!(packets, expected_packets);
            assert_eq!(nonces.len(), expected_packets as usize);
        }
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
}

#[cfg(test)]
mod ptp_sync_tests {
    use super::*;
    #[test]
    fn ptp_time_announcement_has_exact_layout_wrap_and_monotonic_nanoseconds() {
        let time = super::super::ptp::time::Timestamp::new(1, 500_000_000).unwrap();
        let packet = ptp_sync(true, 0x11223344, 11024, 0x0102030405060708, time).unwrap();
        assert_eq!(
            packet,
            [
                0x90, 0xd7, 0, 6, 0x11, 0x22, 0x33, 0x44, 0, 0, 0, 0, 0x59, 0x68, 0x2f, 0, 0xff,
                0xff, 0xff, 0xff, 1, 2, 3, 4, 5, 6, 7, 8,
            ]
        );
        assert_eq!(ptp_sync(false, 0, 0, 1, time).unwrap()[0], 0x80);
        assert!(ptp_sync(true, 0, 0, 0, time).is_err());
        assert!(ptp_sync(
            true,
            0,
            0,
            1,
            super::super::ptp::time::Timestamp {
                seconds: 1 << 48,
                nanos: 0,
            }
        )
        .is_err());
    }
}
