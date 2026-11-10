//! IEEE 1588-2008 wire layout; deliberately bounded, no native struct casting.
use super::time::Timestamp;
use anyhow::{ensure, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Sync = 0,
    DelayReq = 1,
    PdelayReq = 2,
    PdelayResp = 3,
    FollowUp = 8,
    DelayResp = 9,
    PdelayFollowUp = 10,
    Announce = 11,
    Signaling = 12,
}
impl Kind {
    fn parse(n: u8) -> Result<Self> {
        Ok(match n {
            0 => Self::Sync,
            1 => Self::DelayReq,
            2 => Self::PdelayReq,
            3 => Self::PdelayResp,
            8 => Self::FollowUp,
            9 => Self::DelayResp,
            10 => Self::PdelayFollowUp,
            11 => Self::Announce,
            12 => Self::Signaling,
            _ => anyhow::bail!("unsupported PTP message"),
        })
    }
    fn body_len(self) -> usize {
        match self {
            Self::Sync | Self::DelayReq | Self::FollowUp | Self::Signaling => 10,
            Self::Announce => 30,
            _ => 20,
        }
    }
    pub fn event(self) -> bool {
        matches!(
            self,
            Self::Sync | Self::DelayReq | Self::PdelayReq | Self::PdelayResp
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub kind: Kind,
    pub domain: u8,
    pub flags: u16,
    pub correction: i64,
    pub source: [u8; 10],
    pub sequence: u16,
    pub interval: i8,
    pub body: Vec<u8>,
}
impl Packet {
    pub fn new(kind: Kind, clock: u64, sequence: u16, body: Vec<u8>) -> Self {
        let mut source = [0; 10];
        source[..8].copy_from_slice(&clock.to_be_bytes());
        source[8..].copy_from_slice(&0x8005u16.to_be_bytes());
        Self {
            kind,
            domain: 0,
            flags: 0x0400,
            correction: 0,
            source,
            sequence,
            interval: 0,
            body,
        }
    }
    pub fn decode(b: &[u8]) -> Result<Self> {
        ensure!((44..=512).contains(&b.len()), "invalid PTP size");
        ensure!(
            b[0] >> 4 == 1
                && b[1] == 2
                && usize::from(u16::from_be_bytes(b[2..4].try_into()?)) == b.len(),
            "invalid PTP header"
        );
        ensure!(
            b[4] == 0 && b[5] == 0 && b[16..20] == [0; 4],
            "unsupported PTP domain/reserved fields"
        );
        let p = Self {
            kind: Kind::parse(b[0] & 15)?,
            domain: b[4],
            flags: u16::from_be_bytes(b[6..8].try_into()?),
            correction: i64::from_be_bytes(b[8..16].try_into()?),
            source: b[20..30].try_into()?,
            sequence: u16::from_be_bytes(b[30..32].try_into()?),
            interval: b[33] as i8,
            body: b[34..].to_vec(),
        };
        p.validate()?;
        Ok(p)
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            self.domain == 0
                && self.body.len() >= self.kind.body_len()
                && self.body.len() + 34 <= 512,
            "invalid PTP body"
        );
        ensure!(
            self.source[..8] != [0; 8]
                && self.source[..8] != [255; 8]
                && self.source[8..] != [0; 2],
            "invalid source identity"
        );
        ensure!(self.flags & !0x677f == 0, "unsupported PTP flags");
        if self.kind != Kind::Signaling {
            Timestamp::decode(&self.body[..10])?;
        }
        if self.kind == Kind::PdelayReq {
            ensure!(self.body[10..20] == [0; 10], "reserved pdelay bytes");
        }
        let mut tail = &self.body[self.kind.body_len()..];
        while !tail.is_empty() {
            ensure!(tail.len() >= 4, "truncated PTP TLV");
            let n = usize::from(u16::from_be_bytes(tail[2..4].try_into()?));
            ensure!(n % 2 == 0 && n <= tail.len() - 4, "invalid PTP TLV size");
            tail = &tail[n + 4..];
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut b = vec![0; 34];
        b[0] = 0x10 | self.kind as u8;
        b[1] = 2;
        b[2..4].copy_from_slice(&((34 + self.body.len()) as u16).to_be_bytes());
        b[4] = self.domain;
        b[6..8].copy_from_slice(&self.flags.to_be_bytes());
        b[8..16].copy_from_slice(&self.correction.to_be_bytes());
        b[20..30].copy_from_slice(&self.source);
        b[30..32].copy_from_slice(&self.sequence.to_be_bytes());
        b[32] = match self.kind {
            Kind::DelayReq => 1,
            Kind::FollowUp => 2,
            Kind::DelayResp => 3,
            Kind::Sync => 0,
            _ => 5,
        };
        b[33] = self.interval as u8;
        b.extend_from_slice(&self.body);
        Ok(b)
    }
    pub(super) fn kind_body_len(&self) -> usize {
        self.kind.body_len()
    }
    pub fn timestamp(&self) -> Result<Timestamp> {
        ensure!(self.kind != Kind::Signaling, "signaling has no timestamp");
        ensure!(self.body.len() >= 10, "timestamp body too short");
        Timestamp::decode(&self.body[..10])
    }
}
