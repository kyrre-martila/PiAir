//! Type103 transport foundation. No live session entry point.
//! See docs/BUFFERED_TRANSPORT.md and docs/PTP_TIMING.md for lifecycle gates.
mod codec;
use super::{
    crypto,
    planning::{BufferedFormat, Claim},
    profile,
};
use anyhow::{ensure, Context, Result};
pub use codec::{Alac24, BufferedCodec};
use rand::{rngs::OsRng, RngCore};
use std::{
    io::{self, Write},
    net::{Shutdown, TcpStream},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

pub const MAX_FRAME: usize = u16::MAX as usize;
const OVERHEAD: usize = 38;
const SEQUENCE_MASK: u32 = 0x7fffff;

/// Preference order is local policy. Unknown additional IDs cannot veto ALAC24.
/// Authentication/allowlist/topology remain the session/planner's responsibility.
pub fn select_format(formats: &Claim<Vec<BufferedFormat>>) -> Option<BufferedFormat> {
    match formats {
        Claim::Observed(f)
            if f.contains(&BufferedFormat::Alac48000Stereo24)
                && BufferedFormat::Alac48000Stereo24.codec_available() == Claim::Observed(true) =>
        {
            Some(BufferedFormat::Alac48000Stereo24)
        }
        _ => None,
    }
}

/// One fresh key lifetime. Non-cloneable, no imported keys, counters never reset.
/// Move this owner through SETUP and into the transport; drop on any setup error.
pub struct Prepared {
    key: Zeroizing<[u8; 32]>,
    offered: bool,
}
impl Prepared {
    pub fn new() -> Result<Self> {
        let mut key = Zeroizing::new([0; 32]);
        OsRng.try_fill_bytes(key.as_mut())?;
        Ok(Self {
            key,
            offered: false,
        })
    }
    /// Construct once, for an already authenticated control channel. The returned
    /// secret plist must never be logged. This does not perform SETUP.
    pub fn setup_offer(&mut self, connection_id: i64) -> Result<Zeroizing<Vec<u8>>> {
        ensure!(!self.offered, "media key already offered");
        self.offered = true;
        use plist::Value;
        profile::binary(&mut profile::dict([(
            "streams",
            Value::Array(vec![profile::dict([
                ("type", profile::uint(103)),
                ("audioFormat", profile::uint(1 << 21)),
                ("audioFormatIndex", profile::uint(21)),
                ("ct", profile::uint(2)),
                ("spf", profile::uint(codec::FRAMES as u64)),
                ("audioMode", Value::String("default".into())),
                ("shk", Value::Data(self.key.to_vec())),
                ("streamConnectionID", Value::Integer(connection_id.into())),
                ("supportsDynamicStreamID", Value::Boolean(true)),
                (
                    "streamConnections",
                    profile::dict([(
                        "streamConnectionTypeRTP",
                        profile::dict([(
                            "streamConnectionKeyUseStreamEncryptionKey",
                            Value::Boolean(true),
                        )]),
                    )]),
                ),
            ])]),
        )]))
    }
    /// Future authenticated session supplies its authorized, connected data socket.
    /// No network dial, receiver discovery, PTP or playback activation here.
    pub fn attach(self, socket: TcpStream, sequence: u32, timestamp: u32) -> Result<Transport> {
        ensure!(self.offered, "SETUP offer absent");
        ensure!(sequence <= SEQUENCE_MASK, "sequence out of range");
        socket.set_nonblocking(true)?;
        Ok(Transport {
            socket,
            key: self.key,
            nonce: 0,
            sequence,
            timestamp,
            pending: Zeroizing::new(Vec::new()),
            offset: 0,
            failed: false,
        })
    }
}

/// Typed future engine boundary. Implementations must bind these values to the
/// same authenticated session. PTP mapping/control exist in `ptp`; the complete
/// authorized lifecycle and media grant remain gated until PR #42.
pub struct PtpAnchor {
    pub clock_id: u64,
    pub seconds: u64,
    /// Unsigned 64-bit fraction of a network-clock second.
    pub fraction: u64,
    pub sample_time: u32,
}
pub struct NegotiatedStream {
    pub data_port: std::num::NonZeroU16,
}
impl NegotiatedStream {
    /// Called only with the bounded response of the same authenticated SETUP.
    /// Optional echoes must agree; exactly one type103 stream is required.
    pub fn parse(value: &plist::Value, connection_id: i64) -> Result<Self> {
        let root = profile::dictionary(value)?;
        let streams = root
            .get("streams")
            .and_then(plist::Value::as_array)
            .context("streams absent")?;
        ensure!(streams.len() == 1, "expected one buffered stream");
        let stream = profile::dictionary(&streams[0])?;
        ensure!(
            profile::integer(stream, "type")? == 103,
            "unsupported stream type"
        );
        for (field, expected) in [
            ("audioFormat", 1 << 21),
            ("audioFormatIndex", 21),
            ("ct", 2),
            ("spf", codec::FRAMES as u64),
            ("sr", 48_000),
            ("et", 32),
        ] {
            if stream.contains_key(field) {
                ensure!(
                    profile::integer(stream, field)? == expected,
                    "changed buffered codec parameters"
                );
            }
        }
        if let Some(id) = stream.get("streamConnectionID") {
            ensure!(
                id.as_signed_integer() == Some(connection_id),
                "changed stream connection ID"
            );
        }
        Ok(Self {
            data_port: std::num::NonZeroU16::new(profile::port(stream, "dataPort")?)
                .context("zero data port")?,
        })
    }
}
pub trait BufferedLifecycle {
    fn setup(&mut self, offer: &[u8]) -> Result<NegotiatedStream>;
    fn set_peers(&mut self, peers: &[std::net::IpAddr]) -> Result<()>;
    fn set_rate_anchor_time(&mut self, anchor: PtpAnchor) -> Result<()>;
    fn teardown(&mut self) -> Result<()>;
}

/// One outstanding frame is the entire userspace backlog (<=65535 bytes).
/// Enqueue returns false without consuming samples/counters while backpressured.
/// Writes retry the exact ciphertext suffix; no re-encryption after partial I/O.
pub struct Transport {
    socket: TcpStream,
    key: Zeroizing<[u8; 32]>,
    nonce: u64,
    sequence: u32,
    timestamp: u32,
    pending: Zeroizing<Vec<u8>>,
    offset: usize,
    failed: bool,
}
impl Transport {
    pub fn enqueue(&mut self, pcm: &[[i32; 2]], marker: bool) -> Result<bool> {
        ensure!(!self.failed, "transport closed");
        if !self.pending.is_empty() {
            return Ok(false);
        }
        let result = (|| {
            ensure!(self.nonce < u64::MAX, "media nonce exhausted");
            let payload = Alac24.encode(pcm)?;
            self.pending = frame(
                &self.key,
                self.nonce,
                self.sequence,
                self.timestamp,
                marker,
                &payload,
            )?;
            self.nonce += 1;
            self.sequence = (self.sequence + 1) & SEQUENCE_MASK;
            self.timestamp = self.timestamp.wrapping_add(codec::FRAMES as u32);
            Ok(true)
        })();
        if result.is_err() {
            self.abort();
        }
        result
    }
    /// Absolute deadline covers the whole frame, including repeated short writes.
    /// Cancellation is polled at most every 2ms while the socket is stalled.
    pub fn drain(&mut self, deadline: Instant, cancel: &AtomicBool) -> Result<()> {
        ensure!(!self.failed, "transport closed");
        let deadline = deadline.min(Instant::now() + Duration::from_secs(3));
        let result = drain_to(
            &mut self.socket,
            &self.pending,
            &mut self.offset,
            deadline,
            cancel,
        );
        if result.is_err() {
            self.abort();
        } else {
            self.pending.zeroize();
            self.pending.clear();
            self.offset = 0;
        }
        result
    }
    pub fn shutdown(mut self, deadline: Instant, cancel: &AtomicBool) -> Result<()> {
        self.drain(deadline, cancel)?;
        self.socket.shutdown(Shutdown::Write)?;
        self.abort();
        Ok(())
    }
    fn abort(&mut self) {
        self.failed = true;
        self.key.zeroize();
        self.pending.zeroize();
        self.pending.clear();
        self.offset = 0;
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}
impl Drop for Transport {
    fn drop(&mut self) {
        self.abort();
    }
}

fn drain_to(
    writer: &mut impl Write,
    bytes: &[u8],
    offset: &mut usize,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<()> {
    while *offset < bytes.len() {
        ensure!(
            !cancel.load(Ordering::Relaxed),
            "buffered transport cancelled"
        );
        ensure!(Instant::now() < deadline, "buffered write deadline expired");
        match writer.write(&bytes[*offset..]) {
            Ok(0) => anyhow::bail!("buffered receiver disconnected"),
            Ok(n) => *offset += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => (),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::park_timeout(Duration::from_millis(2))
            }
            Err(_) => anyhow::bail!("buffered TCP write failed"),
        }
    }
    Ok(())
}

fn frame(
    key: &[u8; 32],
    counter: u64,
    sequence: u32,
    timestamp: u32,
    marker: bool,
    payload: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(
        !payload.is_empty() && payload.len() <= MAX_FRAME - OVERHEAD,
        "invalid buffered payload length"
    );
    ensure!(sequence <= SEQUENCE_MASK, "invalid buffered sequence");
    let mut header = [0; 12];
    header[..4].copy_from_slice(&(sequence | if marker { 1 << 31 } else { 0 }).to_be_bytes());
    header[4..8].copy_from_slice(&timestamp.to_be_bytes());
    header[8..].copy_from_slice(&0x15000000u32.to_be_bytes());
    let nonce = counter.to_le_bytes();
    let cipher = crypto::seal(key, &crypto::nonce(&nonce), payload, &header[4..])?;
    let mut out = Zeroizing::new(Vec::with_capacity(payload.len() + OVERHEAD));
    out.extend_from_slice(&((payload.len() + OVERHEAD) as u16).to_be_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(&cipher);
    out.extend_from_slice(&nonce);
    Ok(out)
}

/// Bounded complete-frame validator used by mocks and future diagnostics.
/// Sequence/marker are not authenticated by this protocol; never use them as
/// proof of origin. This is not a replay-protected network receiver API.
fn open_frame(key: &[u8; 32], packet: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(
        (OVERHEAD + 1..=MAX_FRAME).contains(&packet.len()),
        "invalid buffered frame size"
    );
    ensure!(
        usize::from(u16::from_be_bytes(packet[..2].try_into()?)) == packet.len(),
        "invalid buffered length"
    );
    let h = &packet[2..14];
    ensure!(
        u32::from_be_bytes(h[..4].try_into()?) & 0x7f800000 == 0,
        "reserved header bits set"
    );
    ensure!(
        h[8..] == 0x15000000u32.to_be_bytes(),
        "unsupported buffered format"
    );
    let nonce: [u8; 8] = packet[packet.len() - 8..]
        .try_into()
        .context("nonce missing")?;
    crypto::open(
        key,
        &crypto::nonce(&nonce),
        &packet[14..packet.len() - 8],
        &h[4..],
    )
}

#[cfg(test)]
mod tests;
