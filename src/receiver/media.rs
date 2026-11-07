//! Sans-I/O receiver-observed buffered packet components. NOT an enabled profile.
//! No sockets, key export/exchange, RTSP negotiation or playback clock here.
//! Evidence, limitations and required Apple TV validation: BUFFERED_RESEARCH.md.
use super::crypto;
use crate::audio::alac;
use anyhow::{ensure, Context, Result};
use rand::{rngs::OsRng, RngCore};
use std::{collections::VecDeque, time::Duration};
use zeroize::{Zeroize, Zeroizing};

pub const MAX_QUEUED_FRAMES: usize = 48_000;
const SEQUENCE_MASK: u32 = 0x00ff_ffff;

pub struct SampleTimeline {
    frames: u64,
    timestamp: u32,
}
impl SampleTimeline {
    pub fn new(timestamp: u32) -> Self {
        Self {
            frames: 0,
            timestamp,
        }
    }
    pub fn timestamp(&self) -> u32 {
        self.timestamp
    }
    pub fn elapsed(&self) -> Duration {
        Duration::new(
            self.frames / u64::from(alac::RATE),
            ((self.frames % u64::from(alac::RATE)) * 1_000_000_000 / u64::from(alac::RATE)) as u32,
        )
    }
    fn advance(&mut self, frames: usize) -> Result<()> {
        ensure!(
            (1..=alac::MAX_FRAMES).contains(&frames),
            "invalid sample increment"
        );
        self.frames = self
            .frames
            .checked_add(frames as u64)
            .context("sample timeline exhausted")?;
        self.timestamp = self.timestamp.wrapping_add(frames as u32);
        Ok(())
    }
}

/// One second hard ceiling, adjustable downwards; backpressure never evicts data.
/// Input is already stereo S16 at 48 kHz. No resampler or activity policy.
pub struct AudioQueue {
    pcm: VecDeque<[i16; 2]>,
    capacity: usize,
    next: u64,
}
impl AudioQueue {
    pub fn new(capacity: usize, first_index: u64) -> Result<Self> {
        ensure!(
            (1..=MAX_QUEUED_FRAMES).contains(&capacity),
            "invalid audio queue capacity"
        );
        Ok(Self {
            pcm: VecDeque::with_capacity(capacity),
            capacity,
            next: first_index,
        })
    }
    /// False means retry the same block after draining; there is no mutation.
    pub fn push(&mut self, index: u64, pcm: &[[i16; 2]]) -> Result<bool> {
        ensure!(
            !pcm.is_empty() && pcm.len() <= alac::MAX_FRAMES,
            "invalid input block size"
        );
        ensure!(index == self.next, "discontinuous audio sample index");
        let next = index
            .checked_add(pcm.len() as u64)
            .context("input sample epoch exhausted")?;
        if pcm.len() > self.capacity - self.pcm.len() {
            return Ok(false);
        }
        self.pcm.extend(pcm.iter().copied());
        self.next = next;
        Ok(true)
    }
    pub fn pop(&mut self, frames: usize) -> Result<Option<Vec<[i16; 2]>>> {
        ensure!(
            (1..=alac::MAX_FRAMES).contains(&frames),
            "invalid audio packet size"
        );
        if self.pcm.len() < frames {
            return Ok(None);
        }
        Ok(Some(self.pcm.drain(..frames).collect()))
    }
    pub fn len(&self) -> usize {
        self.pcm.len()
    }
}
impl Drop for AudioQueue {
    fn drop(&mut self) {
        for frame in &mut self.pcm {
            frame.zeroize();
        }
    }
}

/// Receiver-parser model: 24-bit sequence, timestamp, opaque format word.
/// Its format-word and reserved-bit semantics require a validated profile before
/// any network use. A new instance always generates an independent random key.
pub struct Packetizer {
    key: Zeroizing<[u8; 32]>,
    nonce: u64,
    sequence: u32,
    format_word: u32,
    timeline: SampleTimeline,
    failed: bool,
}
impl Packetizer {
    pub fn new(sequence: u32, timestamp: u32, format_word: u32) -> Result<Self> {
        ensure!(
            sequence <= SEQUENCE_MASK,
            "buffered sequence exceeds 24 bits"
        );
        let mut key = Zeroizing::new([0; 32]);
        OsRng.try_fill_bytes(key.as_mut())?;
        Ok(Self {
            key,
            nonce: 0,
            sequence,
            format_word,
            timeline: SampleTimeline::new(timestamp),
            failed: false,
        })
    }
    /// Successful encoding consumes a nonce even if a future transport drops it.
    /// A packet must never be re-encrypted for retries. Errors invalidate keys.
    pub fn encode(&mut self, pcm: &[[i16; 2]]) -> Result<Zeroizing<Vec<u8>>> {
        let result = self.encode_inner(pcm);
        if result.is_err() {
            self.key.zeroize();
            self.failed = true;
        }
        result
    }
    fn encode_inner(&mut self, pcm: &[[i16; 2]]) -> Result<Zeroizing<Vec<u8>>> {
        ensure!(
            !self.failed && self.nonce < u64::MAX,
            "media key invalid or nonce exhausted"
        );
        let encoded = Zeroizing::new(alac::encode(pcm)?);
        let mut header = [0; 12];
        header[0] = 0x80;
        header[1..4].copy_from_slice(&self.sequence.to_be_bytes()[1..]);
        header[4..8].copy_from_slice(&self.timeline.timestamp().to_be_bytes());
        header[8..12].copy_from_slice(&self.format_word.to_be_bytes());
        let nonce = self.nonce.to_le_bytes();
        let encrypted = crypto::seal(&self.key, &crypto::nonce(&nonce), &encoded, &header[4..12])?;
        let length = u16::try_from(2 + header.len() + encrypted.len() + nonce.len())
            .context("media packet too large")?;
        let mut packet = Zeroizing::new(Vec::with_capacity(usize::from(length)));
        packet.extend_from_slice(&length.to_be_bytes());
        packet.extend_from_slice(&header);
        packet.extend_from_slice(&encrypted); // ciphertext then 16-byte tag
        packet.extend_from_slice(&nonce);
        self.timeline.advance(pcm.len())?;
        self.nonce += 1;
        self.sequence = self.sequence.wrapping_add(1) & SEQUENCE_MASK;
        Ok(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packetizer() -> Packetizer {
        let mut p = Packetizer::new(0xfffffe, 0xfffffffe, 0).unwrap();
        p.key.copy_from_slice(&(0..32).collect::<Vec<_>>());
        p
    }
    fn receive(packet: &[u8], key: &[u8; 32]) -> (u32, u32, Vec<u8>) {
        assert_eq!(
            usize::from(u16::from_be_bytes(packet[..2].try_into().unwrap())),
            packet.len()
        );
        let rtp = &packet[2..];
        assert_eq!(rtp[0], 0x80);
        let sequence = u32::from_be_bytes([0, rtp[1], rtp[2], rtp[3]]);
        let timestamp = u32::from_be_bytes(rtp[4..8].try_into().unwrap());
        let nonce: [u8; 8] = rtp[rtp.len() - 8..].try_into().unwrap();
        let alac = crypto::open(
            key,
            &crypto::nonce(&nonce),
            &rtp[12..rtp.len() - 8],
            &rtp[4..12],
        )
        .unwrap();
        (sequence, timestamp, alac.to_vec())
    }
    #[test]
    fn codec_queue_and_observed_packets_progress_without_nonce_reuse_at_wrap() {
        let mut queue = AudioQueue::new(4, 0).unwrap();
        let pcm = [[-32768, 32767], [0, -1]];
        assert!(queue.push(0, &pcm).unwrap());
        assert!(queue.push(2, &pcm).unwrap());
        assert!(!queue.push(4, &pcm).unwrap());
        assert_eq!(queue.len(), 4);
        let mut p = packetizer();
        let key: [u8; 32] = (0..32).collect::<Vec<_>>().try_into().unwrap();
        for (sequence, timestamp) in [(0xfffffe, 0xfffffffe), (0xffffff, 0), (0, 2)] {
            let samples = queue.pop(2).unwrap().unwrap();
            let packet = p.encode(&samples).unwrap();
            let (seq, ts, payload) = receive(&packet, &key);
            assert_eq!((seq, ts), (sequence, timestamp));
            assert_eq!(alac::tests::decode(&payload), pcm);
            assert!(queue.push(queue.next, &pcm).unwrap());
        }
        assert_eq!(p.nonce, 3);
        assert_eq!(p.timeline.elapsed(), Duration::from_micros(125));
    }
    #[test]
    fn independent_python_packet_fixture_matches_byte_for_byte() {
        let packet = packetizer().encode(&[[-32768, 32767], [0, -1]]).unwrap();
        let fixture = include_bytes!("../../tests/fixtures/buffered-component.bin");
        assert_eq!(packet.as_slice(), fixture);
    }
    #[test]
    fn failures_poison_keys_and_new_sessions_use_fresh_media_keys() {
        let mut p = packetizer();
        p.nonce = u64::MAX;
        assert!(p.encode(&[[0, 0]]).is_err());
        assert!(p.key.iter().all(|b| *b == 0));
        assert!(p.encode(&[[0, 0]]).is_err());
        let mut p = packetizer();
        assert!(p.encode(&[]).is_err());
        assert!(p.encode(&[[0, 0]]).is_err());
        let p = Packetizer::new(0, 0, 0).unwrap();
        let q = Packetizer::new(0, 0, 0).unwrap();
        assert_ne!(p.key.as_ref(), q.key.as_ref());
        assert!(Packetizer::new(0x1000000, 0, 0).is_err());
    }
    #[test]
    fn queue_rejects_gaps_overflow_and_invalid_sizes_without_eviction() {
        assert!(AudioQueue::new(MAX_QUEUED_FRAMES + 1, 0).is_err());
        let mut q = AudioQueue::new(4, 0).unwrap();
        assert!(q.push(1, &[[0, 0]]).is_err());
        assert!(q.push(0, &[]).is_err());
        assert!(q.push(0, &[[1, 2]; 4]).unwrap());
        assert!(!q.push(4, &[[3, 4]]).unwrap());
        assert!(q.pop(0).is_err());
        assert_eq!(q.pop(4).unwrap().unwrap(), [[1, 2]; 4]);
        assert!(q.pop(1).unwrap().is_none());
        let mut q = AudioQueue::new(4, u64::MAX).unwrap();
        assert!(q.push(u64::MAX, &[[0, 0]]).is_err());
    }
    #[test]
    fn sample_time_is_absolute_without_cumulative_roundoff() {
        let mut a = SampleTimeline::new(0);
        let mut b = SampleTimeline::new(0);
        for _ in 0..48_000 {
            a.advance(1).unwrap();
        }
        for _ in 0..48 {
            b.advance(1000).unwrap();
        }
        assert_eq!(a.elapsed(), Duration::from_secs(1));
        assert_eq!(a.elapsed(), b.elapsed());
        assert_eq!(a.timestamp(), b.timestamp());
        a.frames = u64::MAX;
        assert!(a.advance(1).is_err());
    }
}

#[cfg(test)]
mod additional_tests {
    use super::*;
    #[test]
    fn packet_integrity_covers_timestamp_format_word_payload_tag_and_nonce() {
        let mut p = Packetizer::new(17, 123, 456).unwrap();
        let packet = p.encode(&[[1, -1]; 16]).unwrap();
        let key = &p.key;
        // Offsets include the two-byte TCP prefix. Sequence is deliberately not
        // AAD in the observed receiver model; do not invent additional AAD.
        for offset in [6, 10, 14, packet.len() - 9, packet.len() - 1] {
            let mut changed = packet.to_vec();
            changed[offset] ^= 1;
            let rtp = &changed[2..];
            let n: [u8; 8] = rtp[rtp.len() - 8..].try_into().unwrap();
            assert!(crypto::open(
                key,
                &crypto::nonce(&n),
                &rtp[12..rtp.len() - 8],
                &rtp[4..12]
            )
            .is_err());
        }
    }
    #[test]
    fn deterministic_signal_roundtrips_multiple_bounded_blocks_through_independent_codec() {
        let mut queue = AudioQueue::new(352, 0).unwrap();
        let mut p = Packetizer::new(0, 1000, 0).unwrap();
        for block in 0..10_u64 {
            let pcm: Vec<_> = (block * 352..(block + 1) * 352)
                .map(|n| {
                    let left = ((n % 48) as i16 - 24) * 256;
                    [left, -left]
                })
                .collect();
            assert!(queue.push(block * 352, &pcm).unwrap());
            let packet = p.encode(&queue.pop(352).unwrap().unwrap()).unwrap();
            let rtp = &packet[2..];
            let n: [u8; 8] = rtp[rtp.len() - 8..].try_into().unwrap();
            let plain = crypto::open(
                &p.key,
                &crypto::nonce(&n),
                &rtp[12..rtp.len() - 8],
                &rtp[4..12],
            )
            .unwrap();
            assert_eq!(alac::tests::decode(&plain), pcm);
            assert_eq!(queue.len(), 0);
        }
        assert_eq!(p.timeline.timestamp(), 4520);
    }
}
