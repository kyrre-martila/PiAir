//! Bounded PCM WAV input for the realtime diagnostic (no transport logic).
use anyhow::{bail, ensure, Context, Result};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub struct Playback {
    pub frames: Vec<[i16; 2]>,
    pub eof: bool,
}
fn u16le(b: &[u8]) -> u16 {
    u16::from_le_bytes(b[..2].try_into().unwrap())
}
fn u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes(b[..4].try_into().unwrap())
}

impl Playback {
    pub fn open(path: &Path, seconds: u32) -> Result<Self> {
        ensure!(
            (1..=49).contains(&seconds),
            "WAV duration must be 1..49 seconds"
        );
        let mut f = File::open(path).context("open WAV")?;
        let len = f.metadata()?.len();
        let mut header = [0; 12];
        f.read_exact(&mut header).context("truncated WAV header")?;
        ensure!(
            &header[..4] == b"RIFF" && &header[8..] == b"WAVE",
            "expected little-endian RIFF/WAVE"
        );
        let end = u64::from(u32le(&header[4..])) + 8;
        ensure!(end <= len && end >= 12, "truncated RIFF payload");
        let mut format = None;
        let mut data = None;
        while f.stream_position()? + 8 <= end {
            let mut chunk = [0; 8];
            f.read_exact(&mut chunk)?;
            let size = u64::from(u32le(&chunk[4..]));
            let pos = f.stream_position()?;
            ensure!(pos + size + (size & 1) <= end, "truncated WAV chunk");
            match &chunk[..4] {
                b"fmt " => {
                    ensure!(
                        format.is_none() && size >= 16,
                        "invalid or duplicate WAV fmt chunk"
                    );
                    let mut b = [0; 16];
                    f.read_exact(&mut b)?;
                    format = Some(b);
                }
                b"data" => {
                    ensure!(data.is_none(), "duplicate WAV data chunk");
                    data = Some((pos, size));
                }
                _ => {}
            }
            f.seek(SeekFrom::Start(pos + size + (size & 1)))?;
        }
        let b = format.context("WAV fmt chunk missing")?;
        let encoding = u16le(&b);
        ensure!(
            encoding == 1,
            "unsupported WAV encoding {encoding}: only integer PCM (format 1) supported"
        );
        let channels = usize::from(u16le(&b[2..]));
        ensure!(
            channels == 1 || channels == 2,
            "unsupported WAV channel layout: only mono or stereo supported"
        );
        let rate = u32le(&b[4..]);
        ensure!(
            (8000..=192000).contains(&rate),
            "unsupported WAV sample rate {rate}"
        );
        let bits = u16le(&b[14..]);
        ensure!(
            [8, 16, 24, 32].contains(&bits),
            "unsupported PCM bit depth {bits}"
        );
        let width = usize::from(bits / 8);
        let block = channels * width;
        ensure!(
            usize::from(u16le(&b[12..])) == block && u32le(&b[8..]) == rate * block as u32,
            "inconsistent PCM block alignment/byte rate"
        );
        let (offset, bytes) = data.context("WAV data chunk missing")?;
        ensure!(
            bytes % block as u64 == 0 && bytes > 0,
            "empty or partial WAV sample frame"
        );
        let available = bytes / block as u64;
        let requested = u64::from(seconds) * u64::from(rate);
        let count = available.min(requested + 1);
        let mut raw = vec![0; count as usize * block];
        f.seek(SeekFrom::Start(offset))?;
        f.read_exact(&mut raw)?;
        let sample = |frame: usize, channel: usize| -> f64 {
            let start = frame * block + channel * width;
            let s = &raw[start..start + width];
            match bits {
                8 => (f64::from(s[0]) - 128.0) / 128.0,
                16 => f64::from(i16::from_le_bytes(s.try_into().unwrap())) / 32768.0,
                24 => f64::from(i32::from_le_bytes([0, s[0], s[1], s[2]])) / 2147483648.0,
                32 => f64::from(i32::from_le_bytes(s.try_into().unwrap())) / 2147483648.0,
                _ => unreachable!(),
            }
        };
        let length = (available * 44100)
            .div_ceil(u64::from(rate))
            .min(u64::from(seconds) * 44100) as usize;
        if length == 0 {
            bail!("WAV contains no playable frames");
        }
        let frames = (0..length)
            .map(|n| {
                let position = n as f64 * f64::from(rate) / 44100.0;
                let a = position.floor() as usize;
                let b = (a + 1).min(count as usize - 1);
                let fraction = position.fract();
                let fade = n.min(length - 1 - n).min(2205) as f64 / 2205.0;
                std::array::from_fn(|ch| {
                    let ch = ch.min(channels - 1);
                    let x = sample(a, ch) * (1.0 - fraction) + sample(b, ch) * fraction;
                    (x * fade * 32768.0).round().clamp(-32768.0, 32767.0) as i16
                })
            })
            .collect();
        Ok(Self {
            frames,
            eof: available <= requested,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn fixture(
        rate: u32,
        channels: u16,
        bits: u16,
        encoding: u16,
        data: &[u8],
    ) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let block = channels * (bits / 8);
        let mut bytes = b"RIFF".to_vec();
        bytes.extend((36 + data.len() as u32 + (data.len() as u32 & 1)).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(encoding.to_le_bytes());
        bytes.extend(channels.to_le_bytes());
        bytes.extend(rate.to_le_bytes());
        bytes.extend((rate * u32::from(block)).to_le_bytes());
        bytes.extend(block.to_le_bytes());
        bytes.extend(bits.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
        if data.len() % 2 == 1 {
            bytes.push(0);
        }
        f.write_all(&bytes).unwrap();
        f
    }
    #[test]
    fn asset_resamples_and_bounds_without_looping() {
        let p = Playback::open(
            Path::new("tests/assets/audio/Pachelbel_Canon_audio_test.wav"),
            49,
        )
        .unwrap();
        assert_eq!(p.frames.len(), 49 * 44100);
        assert!(!p.eof);
        assert_eq!(p.frames[0], [0, 0]);
        assert_eq!(p.frames.last(), Some(&[0, 0]));
        assert!(p.frames.iter().any(|f| f[0] != f[1]));
    }
    #[test]
    fn integer_depths_mono_eof_and_fades() {
        for (bits, sample) in [
            (8, vec![192]),
            (16, 16384i16.to_le_bytes().to_vec()),
            (24, vec![0, 0, 64]),
            (32, 1073741824i32.to_le_bytes().to_vec()),
        ] {
            let data = sample.repeat(8000);
            let f = fixture(8000, 1, bits, 1, &data);
            let p = Playback::open(f.path(), 2).unwrap();
            assert!(p.eof);
            assert_eq!(p.frames.len(), 44100);
            assert_eq!(p.frames[3000], [16384, 16384]);
            assert_eq!(p.frames[0], [0, 0]);
            assert_eq!(p.frames[44099], [0, 0]);
        }
    }
    #[test]
    fn stereo_order_and_fractional_eof() {
        let data: Vec<_> = (0..4801)
            .flat_map(|_| [1000i16.to_le_bytes(), (-2000i16).to_le_bytes()].concat())
            .collect();
        let f = fixture(48000, 2, 16, 1, &data);
        let p = Playback::open(f.path(), 1).unwrap();
        assert_eq!(p.frames.len(), 4411);
        assert!(p.eof);
        assert!(p.frames[2205][0] > 900 && p.frames[2205][1] < -1900);
    }
    #[test]
    fn malformed_and_unsupported_fail_clearly() {
        for (rate, channels, bits, encoding, data) in [
            (44100, 2, 16, 3, vec![0; 4]),
            (44100, 3, 16, 1, vec![0; 6]),
            (44100, 2, 16, 1, vec![0; 3]),
            (44100, 1, 16, 1, vec![]),
            (0, 1, 16, 1, vec![0; 2]),
        ] {
            let f = fixture(rate, channels, bits, encoding, &data);
            assert!(Playback::open(f.path(), 1).is_err());
        }
        let f = fixture(44100, 1, 16, 1, &[0; 4]);
        f.as_file().set_len(45).unwrap();
        assert!(Playback::open(f.path(), 1).is_err());
        assert!(Playback::open(f.path(), 50).is_err());
    }
}
