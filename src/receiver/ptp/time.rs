//! Integer arithmetic only. PTP seconds are 48 bits; anchors use Q64 fractions.
use anyhow::{ensure, Result};

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
    #[cfg(test)]
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
