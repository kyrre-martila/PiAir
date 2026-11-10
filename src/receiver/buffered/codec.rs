//! Original ALAC escape encoding from Apple's Apache-2.0 format specification.
use anyhow::{ensure, Result};
use zeroize::Zeroizing;

pub const FRAMES: usize = 352;
/// Input is signed, right-aligned S24 stereo. No implicit truncation/resampling.
pub trait BufferedCodec {
    fn encode(&mut self, pcm: &[[i32; 2]]) -> Result<Zeroizing<Vec<u8>>>;
}
pub struct Alac24;
impl BufferedCodec for Alac24 {
    fn encode(&mut self, pcm: &[[i32; 2]]) -> Result<Zeroizing<Vec<u8>>> {
        ensure!(pcm.len() == FRAMES, "expected 352 stereo frames");
        ensure!(
            pcm.iter()
                .flatten()
                .all(|s| (-8_388_608..=8_388_607).contains(s)),
            "sample outside S24 range"
        );
        // CPE, instance 0, reserved 0, explicit sample count, no shifted bytes,
        // escape flag, count, interleaved signed samples, END, zero padding.
        let mut bytes = Zeroizing::new(vec![0; (55 + FRAMES * 48 + 3).div_ceil(8)]);
        let mut bit = 0;
        let mut put = |value: u32, width: usize| {
            for n in (0..width).rev() {
                bytes[bit / 8] |= (((value >> n) & 1) as u8) << (7 - bit % 8);
                bit += 1;
            }
        };
        put(1, 3);
        put(0, 16);
        put(1, 1);
        put(0, 2);
        put(1, 1);
        put(FRAMES as u32, 32);
        for frame in pcm {
            for sample in frame {
                put(*sample as u32, 24);
            }
        }
        put(7, 3);
        Ok(bytes)
    }
}
/// Decoder configuration for independent codec validation; not an RTSP offer.
pub fn cookie() -> [u8; 24] {
    let mut c = [0; 24];
    c[..4].copy_from_slice(&(FRAMES as u32).to_be_bytes());
    c[5..10].copy_from_slice(&[24, 40, 10, 14, 2]);
    c[10..12].copy_from_slice(&255u16.to_be_bytes());
    c[20..].copy_from_slice(&48_000u32.to_be_bytes());
    c
}
