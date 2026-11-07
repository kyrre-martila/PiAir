//! Original ALAC escape-frame encoder. Format evidence: BUFFERED_RESEARCH.md.
//! Codec validity does not establish negotiated AirPlay buffered ALAC support.
use anyhow::{ensure, Result};

pub const RATE: u32 = 48_000;
pub const MAX_FRAMES: usize = 4096;

/// Signed normalized PCM -> S16, round to nearest, ties away from zero.
/// Both channels remain independent; invalid input is never replaced by silence.
pub fn from_normalized(pcm: [f32; 2]) -> Result<[i16; 2]> {
    ensure!(
        pcm.iter()
            .all(|x| x.is_finite() && (-1.0..=1.0).contains(x)),
        "invalid normalized PCM"
    );
    Ok(pcm.map(|x| (x * 32768.0).round().clamp(-32768.0, 32767.0) as i16))
}

/// Apple's 24-byte ALACSpecificConfig, stereo 48 kHz / 16 bit.
/// This is decoder configuration, not an AirPlay audioFormat/ct offer.
pub fn cookie() -> [u8; 24] {
    let mut cookie = [0; 24];
    cookie[..4].copy_from_slice(&(MAX_FRAMES as u32).to_be_bytes());
    cookie[5..10].copy_from_slice(&[16, 40, 10, 14, 2]);
    cookie[10..12].copy_from_slice(&255_u16.to_be_bytes());
    cookie[20..].copy_from_slice(&RATE.to_be_bytes());
    cookie
}

/// Encode one independently decodable channel-pair element plus END and padding.
/// Samples are bit-packed MSB first in left/right order, independent of host endian.
/// Escape coding is lossless ALAC without predictive/entropy compression.
pub fn encode(pcm: &[[i16; 2]]) -> Result<Vec<u8>> {
    ensure!(
        (1..=MAX_FRAMES).contains(&pcm.len()),
        "ALAC block must contain 1..4096 stereo frames"
    );
    let explicit_count = pcm.len() != MAX_FRAMES;
    let bits = 23 + usize::from(explicit_count) * 32 + pcm.len() * 32 + 3;
    let mut out = Bits {
        bytes: vec![0; bits.div_ceil(8)],
        next: 0,
    };
    out.put(1, 3); // channel-pair element (CPE)
    out.put(0, 4); // element instance
    out.put(0, 12); // reserved
    out.put(u32::from(explicit_count), 1);
    out.put(0, 2); // no shifted-off bytes
    out.put(1, 1); // escape (verbatim) coding
    if explicit_count {
        out.put(pcm.len() as u32, 32);
    }
    for frame in pcm {
        for sample in frame {
            out.put(u32::from(*sample as u16), 16);
        }
    }
    out.put(7, 3); // END element; remaining alignment bits are zero
    Ok(out.bytes)
}
struct Bits {
    bytes: Vec<u8>,
    next: usize,
}
impl Bits {
    fn put(&mut self, value: u32, count: u32) {
        for bit in (0..count).rev() {
            self.bytes[self.next / 8] |= (((value >> bit) & 1) as u8) << (7 - self.next % 8);
            self.next += 1;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use symphonia_codec_alac::AlacDecoder;
    use symphonia_core::{
        audio::Signal,
        codecs::{CodecParameters, Decoder, DecoderOptions, CODEC_TYPE_ALAC},
        formats::Packet,
    };
    pub(crate) fn decode(packet: &[u8]) -> Vec<[i16; 2]> {
        let mut params = CodecParameters::new();
        params
            .for_codec(CODEC_TYPE_ALAC)
            .with_extra_data(Box::new(cookie()));
        let mut decoder = AlacDecoder::try_new(&params, &DecoderOptions::default()).unwrap();
        let input = Packet::new_from_slice(0, 0, 0, packet);
        let decoded = decoder.decode(&input).unwrap();
        assert_eq!(decoded.spec().rate, RATE);
        assert_eq!(decoded.spec().channels.count(), 2);
        let mut samples = symphonia_core::audio::AudioBuffer::<i16>::new(
            decoded.capacity() as u64,
            *decoded.spec(),
        );
        decoded.convert(&mut samples);
        samples
            .chan(0)
            .iter()
            .copied()
            .zip(samples.chan(1).iter().copied())
            .map(|(l, r)| [l, r])
            .collect()
    }
    #[test]
    fn independent_decoder_preserves_stereo_sign_extremes_and_packet_lengths() {
        for length in [1, 2, 352, 1024, 4095, MAX_FRAMES] {
            let pcm: Vec<_> = (0..length)
                .map(|i| [i.wrapping_mul(7919) as i16, !(i.wrapping_mul(3137) as i16)])
                .collect();
            let packet = encode(&pcm).unwrap();
            assert_eq!(decode(&packet), pcm);
            assert!(packet.len() <= MAX_FRAMES * 4 + 8);
        }
        let pcm = [[i16::MIN, i16::MAX], [0, -1], [1, 0]];
        assert_eq!(decode(&encode(&pcm).unwrap()), pcm);
        assert!(encode(&[]).is_err());
        assert!(encode(&vec![[0; 2]; MAX_FRAMES + 1]).is_err());
    }
    #[test]
    fn conversion_does_not_swap_channels_or_accept_invalid_samples() {
        assert_eq!(from_normalized([-1.0, 1.0]).unwrap(), [i16::MIN, i16::MAX]);
        assert_eq!(from_normalized([0.5, -0.5]).unwrap(), [16384, -16384]);
        for bad in [f32::NAN, f32::INFINITY, -1.01, 1.01] {
            assert!(from_normalized([bad, 0.0]).is_err());
        }
    }
}
