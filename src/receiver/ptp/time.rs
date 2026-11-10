//! Integer arithmetic only. PTP seconds are 48 bits; anchors use Q64 fractions.
use anyhow::{ensure, Context, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timestamp {
    pub seconds: u64,
    pub nanos: u32,
}
impl Timestamp {
    pub fn new(seconds: u64, nanos: u32) -> Result<Self> {
        ensure!(
            seconds < 1 << 48 && nanos < 1_000_000_000,
            "invalid PTP timestamp"
        );
        Ok(Self { seconds, nanos })
    }
    pub fn decode(b: &[u8]) -> Result<Self> {
        ensure!(b.len() == 10, "invalid timestamp length");
        let mut seconds = [0; 8];
        seconds[2..].copy_from_slice(&b[..6]);
        Self::new(
            u64::from_be_bytes(seconds),
            u32::from_be_bytes(b[6..].try_into()?),
        )
    }
    pub fn encode(self) -> Result<[u8; 10]> {
        Self::new(self.seconds, self.nanos)?;
        let mut b = [0; 10];
        b[..6].copy_from_slice(&self.seconds.to_be_bytes()[2..]);
        b[6..].copy_from_slice(&self.nanos.to_be_bytes());
        Ok(b)
    }
    pub fn ns(self) -> i128 {
        i128::from(self.seconds) * 1_000_000_000 + i128::from(self.nanos)
    }
    pub fn from_ns(ns: i128) -> Result<Self> {
        ensure!(ns >= 0, "negative PTP time");
        Self::new(
            u64::try_from(ns / 1_000_000_000)?,
            (ns % 1_000_000_000) as u32,
        )
    }
    pub fn network(self) -> NetworkTime {
        NetworkTime {
            seconds: self.seconds,
            fraction: ((u128::from(self.nanos) << 64) / 1_000_000_000) as u64,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkTime {
    pub seconds: u64,
    pub fraction: u64,
}
impl NetworkTime {
    fn ticks(self) -> Result<i128> {
        ensure!(self.seconds < 1 << 48, "network time outside PTP era");
        Ok((i128::from(self.seconds) << 64) + i128::from(self.fraction))
    }
    fn from_ticks(ticks: i128) -> Result<Self> {
        ensure!(
            (0..(1i128 << 112)).contains(&ticks),
            "network time overflow"
        );
        Ok(Self {
            seconds: (ticks >> 64) as u64,
            fraction: ticks as u64,
        })
    }
}
/// Absolute sample positions avoid repeated rounding and ambiguity at RTP wrap.
/// Changing a timeline requires constructing a new mapping with a new generation.
pub struct SampleTimeline {
    pub clock_id: u64,
    pub generation: u64,
    origin: NetworkTime,
    sample: u64,
}
impl SampleTimeline {
    pub fn new(clock_id: u64, generation: u64, origin: NetworkTime, sample: u64) -> Result<Self> {
        ensure!(
            clock_id != 0 && clock_id != u64::MAX,
            "invalid clock identity"
        );
        origin.ticks()?;
        Ok(Self {
            clock_id,
            generation,
            origin,
            sample,
        })
    }
    pub fn at(&self, sample: u64) -> Result<NetworkTime> {
        let delta = i128::from(sample) - i128::from(self.sample);
        // Bound before multiplication; a single mapping may span at most ~1462 years.
        ensure!(
            delta.unsigned_abs() <= (1u128 << 51),
            "sample span too large"
        );
        let ticks = delta
            .checked_mul(1i128 << 64)
            .context("sample conversion overflow")?
            .div_euclid(48_000);
        NetworkTime::from_ticks(
            self.origin
                .ticks()?
                .checked_add(ticks)
                .context("time overflow")?,
        )
    }
    /// Nearest sample, ties toward the later sample. Q64 error is < 2^-64 seconds.
    pub fn sample_at(&self, time: NetworkTime) -> Result<u64> {
        let delta = time.ticks()? - self.origin.ticks()?;
        let samples = delta
            .checked_mul(48_000)
            .context("sample conversion overflow")?
            .checked_add(1i128 << 63)
            .context("sample rounding overflow")?
            .div_euclid(1i128 << 64);
        Ok(u64::try_from(i128::from(self.sample) + samples)?)
    }
}

/// IEEE 1588 end-to-end symmetric path solution, in 2^-16 nanoseconds.
/// t1/t4 are master times; t2/t3 local times. Corrections retain signed fractions.
#[derive(Clone, Copy, Debug)]
pub struct Measurement {
    pub(super) offset_scaled: i128,
    pub(super) delay_scaled: i128,
    pub(super) local_ns: i128,
}
impl Measurement {
    pub fn solve(t: [Timestamp; 4], corrections: [i64; 2]) -> Result<Self> {
        for stamp in t {
            Timestamp::new(stamp.seconds, stamp.nanos)?;
        }
        let [t1, t2, t3, t4] = t.map(Timestamp::ns);
        ensure!(t3 >= t2 && t4 >= t1, "inconsistent exchange");
        let forward = (t2 - t1) * 65_536 - i128::from(corrections[0]);
        let reverse = (t4 - t3) * 65_536 - i128::from(corrections[1]);
        let delay_scaled = (forward + reverse) / 2;
        ensure!(
            (0..=100_000_000i128 * 65_536).contains(&delay_scaled),
            "invalid path delay"
        );
        Ok(Self {
            offset_scaled: (forward - reverse) / 2,
            delay_scaled,
            local_ns: t2,
        })
    }
}
