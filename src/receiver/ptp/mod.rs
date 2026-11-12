//! Session-owned PTP foundation. No discovery, dialing, threads or system clock writes.
//! The fixed grandmaster role does not claim receiver servo lock.
pub mod control;
pub mod packet;
pub mod session;
pub mod time;
use anyhow::{ensure, Context, Result};
use packet::{Kind, Packet};
use std::{
    io,
    net::UdpSocket,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant},
};
use time::{Measurement, NetworkTime, Timestamp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Initializing,
    Synchronizing,
    PeerObserved,
    Locked,
    Lost,
    Shutdown,
}

/// Independent follower estimator; not used to elect a receiver as grandmaster.
/// The owner must correlate all four timestamps and identities before submission.
pub struct Estimator {
    state: State,
    generation: u64,
    first: Option<Measurement>,
    last: Option<Measurement>,
    count: u8,
    drift_ppb: i128,
}
impl Estimator {
    pub fn new() -> Self {
        Self {
            state: State::Initializing,
            generation: 0,
            first: None,
            last: None,
            count: 0,
            drift_ppb: 0,
        }
    }
    pub fn invalidate(&mut self) {
        self.state = State::Lost;
        self.first = None;
        self.last = None;
        self.count = 0;
        self.generation = self.generation.saturating_add(1);
    }
    pub fn observe(&mut self, m: Measurement) -> Result<()> {
        ensure!(self.state != State::Shutdown, "estimator shut down");
        if let Some(last) = self.last {
            let dt = m.local_ns - last.local_ns;
            if !(1_000_000..=3_000_000_000).contains(&dt) {
                self.invalidate();
                anyhow::bail!("stale or discontinuous exchange");
            }
            let change = m.offset_scaled - last.offset_scaled;
            if change.abs() > 5_000_000 * 65_536 {
                self.invalidate();
                anyhow::bail!("clock offset discontinuity");
            }
        }
        let first = *self.first.get_or_insert(m);
        let span = m.local_ns - first.local_ns;
        if span > 0 {
            self.drift_ppb =
                (m.offset_scaled - first.offset_scaled) * 1_000_000_000 / (span * 65_536);
            if self.drift_ppb.abs() > 500_000 {
                self.invalidate();
                anyhow::bail!("clock drift outside bound");
            }
        }
        self.count = self.count.saturating_add(1);
        self.last = Some(m);
        self.state = if self.count >= 4 && span >= 1_000_000_000 {
            State::Locked
        } else {
            State::Synchronizing
        };
        Ok(())
    }
    pub fn network_at(&mut self, local_ns: i128) -> Result<Timestamp> {
        let last = self.last.context("no timing evidence")?;
        let dt = local_ns - last.local_ns;
        if !(0..=3_000_000_000).contains(&dt) {
            self.invalidate();
        }
        ensure!(self.state == State::Locked, "clock not locked");
        Timestamp::from_ns(
            local_ns - last.offset_scaled / 65_536 - dt * self.drift_ppb / 1_000_000_000,
        )
    }
    pub fn shutdown(&mut self) {
        self.invalidate();
        self.state = State::Shutdown;
    }
}

// Process-local generation IDs prevent stale mappings surviving owner recreation.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
fn next_generation() -> Result<u64> {
    let mut n = NEXT_GENERATION.load(Ordering::Relaxed);
    loop {
        let next = n.checked_add(1).context("clock generation exhausted")?;
        match NEXT_GENERATION.compare_exchange_weak(n, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(n),
            Err(current) => n = current,
        }
    }
}

/// A fixed single-peer grandmaster, using an explicitly chosen local epoch.
/// All incoming datagrams must come from an already authorized connected socket.
/// Fresh validated peer traffic is liveness, never remote offset or lock evidence.
pub struct Grandmaster {
    clock_id: u64,
    peer_clock_id: u64,
    state: State,
    generation: u64,
    epoch: Instant,
    deadline: Instant,
    last_tick: Instant,
    next_sync: Instant,
    next_announce: Instant,
    sequence: u16,
    announce_sequence: u16,
    pending_sync: Option<(u16, Instant)>,
    last_sequences: [Option<u16>; 16],
    peer_ports: [Option<u16>; 16],
    incoming_sync: Option<(u16, u16, Instant)>,
    last_peer: Option<Instant>,
}
impl Grandmaster {
    pub fn new(
        clock_id: u64,
        peer_clock_id: u64,
        now: Instant,
        setup_timeout: Duration,
    ) -> Result<Self> {
        ensure!(
            clock_id != 0 && clock_id != u64::MAX,
            "invalid clock identity"
        );
        ensure!(
            peer_clock_id != 0 && peer_clock_id != u64::MAX && peer_clock_id != clock_id,
            "invalid peer clock identity"
        );
        ensure!(
            !setup_timeout.is_zero() && setup_timeout <= Duration::from_secs(30),
            "invalid setup timeout"
        );
        Ok(Self {
            clock_id,
            peer_clock_id,
            state: State::Initializing,
            generation: next_generation()?,
            epoch: now,
            deadline: now + setup_timeout,
            last_tick: now,
            next_sync: now,
            next_announce: now,
            sequence: 0,
            announce_sequence: 0,
            pending_sync: None,
            last_sequences: [None; 16],
            peer_ports: [None; 16],
            incoming_sync: None,
            last_peer: None,
        })
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    fn lose(&mut self) {
        self.state = State::Lost;
    }
    pub fn check(&mut self, now: Instant, cancelled: bool) -> Result<()> {
        if cancelled {
            self.shutdown();
        }
        ensure!(self.state != State::Shutdown, "PTP shutdown/cancelled");
        if now < self.last_tick
            || now.duration_since(self.last_tick) > Duration::from_secs(3)
            || self
                .last_peer
                .is_some_and(|p| now.saturating_duration_since(p) > Duration::from_secs(3))
            || (self.last_peer.is_none() && now >= self.deadline)
        {
            self.lose();
        }
        self.last_tick = now;
        ensure!(
            self.state != State::Lost,
            "PTP timing evidence expired; recreate session clock"
        );
        if self.state == State::Initializing {
            self.state = State::Synchronizing;
        }
        Ok(())
    }
    pub fn network_time(&self, now: Instant) -> Result<NetworkTime> {
        ensure!(
            matches!(self.state, State::Synchronizing | State::PeerObserved),
            "clock unavailable"
        );
        ensure!(
            now >= self.last_tick
                && now.saturating_duration_since(self.last_tick) <= Duration::from_secs(3)
                && self
                    .last_peer
                    .map_or(now < self.deadline, |p| now.saturating_duration_since(p)
                        <= Duration::from_secs(3)),
            "stale network clock"
        );
        let d = now
            .checked_duration_since(self.epoch)
            .context("monotonic discontinuity")?;
        Ok(Timestamp::new(d.as_secs(), d.subsec_nanos())?.network())
    }
    fn timestamp(&self, now: Instant) -> Result<Timestamp> {
        let d = now
            .checked_duration_since(self.epoch)
            .context("monotonic discontinuity")?;
        Timestamp::new(d.as_secs(), d.subsec_nanos())
    }
    /// Call after an actual Sync send; matching FollowUp names that send time.
    pub fn follow_up(&mut self, sync: &Packet, sent: Instant) -> Result<Packet> {
        ensure!(
            sync.kind == Kind::Sync && sync.source[..8] == self.clock_id.to_be_bytes(),
            "unmatched sync"
        );
        self.network_time(sent)?;
        let (sequence, scheduled) = self.pending_sync.context("no outstanding Sync")?;
        ensure!(
            sequence == sync.sequence
                && sent >= scheduled
                && sent.saturating_duration_since(scheduled) <= Duration::from_millis(125),
            "stale Sync send"
        );
        self.pending_sync = None;
        let mut p = Packet::new(
            Kind::FollowUp,
            self.clock_id,
            sync.sequence,
            self.timestamp(sent)?.encode()?.to_vec(),
        );
        // IEEE 802.1AS Follow_Up information TLV: local oscillator has no
        // rate offset, GM change or phase change relative to its own timeline.
        p.body
            .extend_from_slice(&[0, 3, 0, 28, 0, 0x80, 0xc2, 0, 0, 1]);
        p.body.extend_from_slice(&[0; 22]);
        // Empirically corroborated Apple clock identity extension. Length 16
        // includes OUI/subtype (6), clock (8) and two reserved zero bytes.
        p.body
            .extend_from_slice(&[0, 3, 0, 16, 0, 0x0d, 0x93, 0, 0, 4]);
        p.body.extend_from_slice(&self.clock_id.to_be_bytes());
        p.body.extend_from_slice(&[0; 2]);
        p.interval = -3;
        Ok(p)
    }
    pub fn due(&mut self, now: Instant) -> Result<Vec<Packet>> {
        self.check(now, false)?;
        let mut out = Vec::with_capacity(2);
        if now >= self.next_announce {
            // Honest free-running oscillator: no GPS, UTC or traceability claim.
            let mut b = vec![0; 30];
            b[13] = 128;
            b[14] = 248;
            b[15] = 0xfe;
            b[16..18].copy_from_slice(&0xffffu16.to_be_bytes());
            b[18] = 128;
            b[19..27].copy_from_slice(&self.clock_id.to_be_bytes());
            b[29] = 0xa0;
            // Standard PATH_TRACE TLV with this grandmaster only.
            b.extend_from_slice(&[0, 8, 0, 8]);
            b.extend_from_slice(&self.clock_id.to_be_bytes());
            out.push(Packet::new(
                Kind::Announce,
                self.clock_id,
                self.announce_sequence,
                b,
            ));
            self.announce_sequence = self.announce_sequence.wrapping_add(1);
            self.next_announce = now + Duration::from_secs(1);
        }
        if now >= self.next_sync {
            let mut p = Packet::new(Kind::Sync, self.clock_id, self.sequence, vec![0; 10]);
            p.flags |= 0x0200;
            p.interval = -3;
            out.push(p);
            self.pending_sync = Some((self.sequence, now));
            self.sequence = self.sequence.wrapping_add(1);
            self.next_sync = now + Duration::from_millis(125);
        }
        Ok(out)
    }
    pub fn receive(&mut self, bytes: &[u8], received: Instant) -> Result<Vec<Packet>> {
        self.check(received, false)?;
        let p = Packet::decode(bytes)?;
        ensure!(
            p.source[..8] != self.clock_id.to_be_bytes(),
            "reflected local clock"
        );
        ensure!(
            p.source[..8] == self.peer_clock_id.to_be_bytes(),
            "unexpected peer clock identity"
        );
        ensure!(p.kind.event(), "general packet on event channel");
        if p.kind == Kind::Sync {
            self.validate_observation(&p, received)?;
            self.incoming_sync = if p.flags & 0x0200 != 0 {
                Some((p.port_number(), p.sequence, received))
            } else {
                None
            };
            return Ok(Vec::new());
        }
        ensure!(p.correction == 0, "unsupported corrected delay request");
        let slot = match p.kind {
            Kind::DelayReq => 0,
            Kind::PdelayReq => 1,
            _ => anyhow::bail!("unsupported message for fixed grandmaster"),
        };
        ensure!(
            p.body.len() == p.kind_body_len() && p.flags & !0x0408 == 0,
            "unsupported request extensions/flags"
        );
        self.validate_observation(&p, received)?;
        let mut body = self.timestamp(received)?.encode()?.to_vec();
        body.extend_from_slice(&p.source);
        let kind = if slot == 0 {
            Kind::DelayResp
        } else {
            Kind::PdelayResp
        };
        let mut response = Packet::new(kind, self.clock_id, p.sequence, body);
        if slot == 1 {
            response.flags |= 0x0200;
        }
        Ok(vec![response])
    }
    /// Pin each message role independently: a clock can expose multiple PTP
    /// ports. Only the Sync/Follow_Up pair requires identical port and sequence.
    /// This never runs a servo or a best-master election.
    fn validate_observation(&mut self, p: &Packet, received: Instant) -> Result<()> {
        ensure!(
            p.source[..8] == self.peer_clock_id.to_be_bytes(),
            "unexpected peer clock identity"
        );
        let slot = if p.kind == Kind::FollowUp {
            Kind::Sync as usize
        } else {
            p.kind as usize
        };
        let port = p.port_number();
        if let Some(expected) = self.peer_ports[slot] {
            ensure!(port == expected, "peer role port identity changed");
        }
        if let Some(last) = self.last_sequences[p.kind as usize] {
            let delta = p.sequence.wrapping_sub(last);
            ensure!(delta > 0 && delta < 0x8000, "stale/replayed PTP sequence");
        }
        if p.kind == Kind::FollowUp {
            let (sync_port, sequence, at) = self.incoming_sync.context("unmatched Follow_Up")?;
            ensure!(
                port == sync_port
                    && p.sequence == sequence
                    && received.saturating_duration_since(at) <= Duration::from_secs(3),
                "unmatched/stale Follow_Up"
            );
        }
        self.peer_ports[slot] = Some(port);
        self.last_sequences[p.kind as usize] = Some(p.sequence);
        if p.kind == Kind::FollowUp {
            self.incoming_sync = None;
        }
        self.last_peer = Some(received);
        self.state = State::PeerObserved;
        Ok(())
    }
    /// Signaling TLVs are bounded by Packet::decode and remain inert: no peer
    /// registration, destination, authorization or timing policy changes.
    fn receive_general(&mut self, bytes: &[u8], received: Instant) -> Result<()> {
        self.check(received, false)?;
        let p = Packet::decode(bytes)?;
        ensure!(
            matches!(p.kind, Kind::FollowUp | Kind::Announce | Kind::Signaling),
            "unsupported general observation"
        );
        if p.kind == Kind::Signaling {
            let mut local = [0; 10];
            local[..8].copy_from_slice(&self.clock_id.to_be_bytes());
            local[8..].copy_from_slice(&0x8005u16.to_be_bytes());
            ensure!(
                p.body[..10] == local || p.body[..10] == [0xff; 10] || p.body[..10] == [0; 10],
                "unexpected signaling target"
            );
        }
        self.validate_observation(&p, received)
    }
    pub fn shutdown(&mut self) {
        self.state = State::Shutdown;
        self.pending_sync = None;
        self.last_sequences = [None; 16];
        self.peer_ports = [None; 16];
        self.incoming_sync = None;
        self.last_peer = None;
    }
    /// PR #42 must supply a complete session grant; timing alone never grants media.
    pub fn media_permitted(&self) -> bool {
        false
    }
}
impl Drop for Grandmaster {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Successful socket sends and validated pinned-peer receives, never servo evidence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Evidence {
    pub sync_sent: u64,
    pub follow_up_sent: u64,
    pub announce_sent: u64,
    pub delay_requests: u64,
    pub delay_responses_sent: u64,
    pub pdelay_requests: u64,
    pub general_received: u64,
    pub sync_received: u64,
    pub follow_up_received: u64,
    pub announce_received: u64,
    pub signaling_received: u64,
    pub rejected: u64,
}
/// No socket creation: Wire supplies connected event/general sockets for the
/// selected authenticated receiver (UDP 319/320). Dropping releases both sockets.
/// Poll on the session thread; no worker/service survives the session owner.
pub struct Service {
    clock: Grandmaster,
    event: Option<UdpSocket>,
    general: Option<UdpSocket>,
    lease: super::wire::PtpLease,
    diagnostics: Diagnostics,
    evidence: Evidence,
}
// Separate lifetime budgets: rejected floods cannot hide successful exchanges.
// Only bounded header identities; never payloads, addresses or control-session data.
#[derive(Default)]
struct Diagnostics {
    rejected: u8,
    exchanges: u8,
}
impl Diagnostics {
    const LIMIT: u8 = 8;
    fn reject(&mut self, channel: &'static str, bytes: &[u8], reason: &anyhow::Error) {
        if self.rejected >= Self::LIMIT {
            return;
        }
        self.rejected += 1;
        tracing::debug!(target: "piair::ptp", channel, length = bytes.len(),
            message_type = ?bytes.first().map(|b| b & 15),
            clock_identity = ?bytes.get(20..28).map(|b| format!("{:016x}", u64::from_be_bytes(b.try_into().expect("bounded identity")))),
            port_number = ?bytes.get(28..30).map(|b| u16::from_be_bytes(b.try_into().expect("bounded port"))),
            control = ?bytes.get(32), reason = %reason,
            diagnostic = self.rejected, "rejected PTP datagram");
    }
    fn exchange(&mut self, sequence: u16) {
        if self.exchanges >= Self::LIMIT {
            return;
        }
        self.exchanges += 1;
        tracing::debug!(target: "piair::ptp", sequence, channel = "general",
            diagnostic = self.exchanges,
            "accepted Delay_Req; sent Delay_Resp");
    }
}
impl Service {
    pub(super) fn attach(
        clock: Grandmaster,
        event: UdpSocket,
        general: UdpSocket,
        lease: super::wire::PtpLease,
    ) -> Result<Self> {
        lease.check()?;
        let e = event.peer_addr()?;
        let g = general.peer_addr()?;
        ensure!(
            e.ip() == lease.peer() && g.ip() == lease.peer(),
            "PTP peer does not match authorization"
        );
        event.set_nonblocking(true)?;
        general.set_nonblocking(true)?;
        Ok(Self {
            clock,
            event: Some(event),
            general: Some(general),
            lease,
            diagnostics: Diagnostics::default(),
            evidence: Evidence::default(),
        })
    }
    fn check_authorization(&mut self) -> Result<()> {
        let result = self.lease.check();
        if result.is_err() {
            self.shutdown();
        }
        result
    }
    fn send(&mut self, p: &Packet) -> Result<()> {
        self.check_authorization()?;
        let b = p.encode()?;
        let socket = if p.kind.event() {
            &self.event
        } else {
            &self.general
        };
        let socket = socket.as_ref().context("PTP socket closed")?;
        ensure!(socket.send(&b)? == b.len(), "partial PTP datagram");
        match p.kind {
            Kind::Sync => self.evidence.sync_sent += 1,
            Kind::FollowUp => self.evidence.follow_up_sent += 1,
            Kind::Announce => self.evidence.announce_sent += 1,
            Kind::DelayResp | Kind::PdelayResp => self.evidence.delay_responses_sent += 1,
            _ => (),
        }
        Ok(())
    }
    pub fn poll(&mut self, cancel: &AtomicBool) -> Result<()> {
        let result = self.poll_inner(cancel);
        if result.is_err() {
            self.shutdown();
        }
        result
    }
    fn poll_inner(&mut self, cancel: &AtomicBool) -> Result<()> {
        self.check_authorization()?;
        self.clock
            .check(Instant::now(), cancel.load(Ordering::Relaxed))?;
        for p in self.clock.due(Instant::now())? {
            self.send(&p)?;
            if p.kind == Kind::Sync {
                let follow_up = self.clock.follow_up(&p, Instant::now())?;
                self.send(&follow_up)?;
            }
        }
        // Bounded work even under a flood. Wrong-port packets cannot refresh liveness.
        for _ in 0..16 {
            self.check_authorization()?;
            if cancel.load(Ordering::Relaxed) {
                self.clock.shutdown();
                anyhow::bail!("PTP cancelled");
            }
            let mut b = [0; 513];
            match self
                .event
                .as_ref()
                .context("PTP socket closed")?
                .recv(&mut b)
            {
                Ok(n) => {
                    self.check_authorization()?;
                    // Invalid unauthenticated UDP is ignored, never a lock signal.
                    match self.clock.receive(&b[..n], Instant::now()) {
                        Err(reason) => {
                            self.evidence.rejected += 1;
                            self.diagnostics.reject("event", &b[..n], &reason);
                        }
                        Ok(responses) => {
                            if responses.is_empty() {
                                self.evidence.sync_received += 1;
                            } else {
                                if responses[0].kind == Kind::PdelayResp {
                                    self.evidence.pdelay_requests += 1;
                                } else {
                                    self.evidence.delay_requests += 1;
                                }
                            }
                            for p in responses {
                                self.send(&p)?;
                                if p.kind == Kind::DelayResp {
                                    self.diagnostics.exchange(p.sequence);
                                }
                                if p.kind == Kind::PdelayResp {
                                    let mut body =
                                        self.clock.timestamp(Instant::now())?.encode()?.to_vec();
                                    body.extend_from_slice(&p.body[10..20]);
                                    self.send(&Packet::new(
                                        Kind::PdelayFollowUp,
                                        self.clock.clock_id,
                                        p.sequence,
                                        body,
                                    ))?;
                                }
                            }
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        // Drain general traffic too, with a separate fixed work bound. No
        // private TLV can register peers or authorize additional destinations.
        for _ in 0..16 {
            self.check_authorization()?;
            ensure!(!cancel.load(Ordering::Relaxed), "PTP cancelled");
            let mut bytes = [0; 513];
            match self
                .general
                .as_ref()
                .context("PTP socket closed")?
                .recv(&mut bytes)
            {
                Ok(n) => {
                    self.check_authorization()?;
                    if let Err(reason) = self.clock.receive_general(&bytes[..n], Instant::now()) {
                        self.evidence.rejected += 1;
                        self.diagnostics.reject("general", &bytes[..n], &reason);
                    } else {
                        self.evidence.general_received += 1;
                        match bytes[0] & 15 {
                            8 => self.evidence.follow_up_received += 1,
                            11 => self.evidence.announce_received += 1,
                            12 => self.evidence.signaling_received += 1,
                            _ => (),
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        self.check_authorization()
    }
    pub fn shutdown(&mut self) {
        self.clock.shutdown();
        self.event.take();
        self.general.take();
    }
    pub fn evidence(&self) -> Evidence {
        self.evidence
    }
    pub(super) fn peer(&self) -> std::net::IpAddr {
        self.lease.peer()
    }
    pub fn clock(&self) -> &Grandmaster {
        &self.clock
    }
    pub fn clock_mut(&mut self) -> &mut Grandmaster {
        &mut self.clock
    }
    pub fn state(&self) -> State {
        self.clock.state()
    }
}

#[cfg(test)]
mod tests;

impl Drop for Service {
    fn drop(&mut self) {
        self.shutdown();
    }
}
