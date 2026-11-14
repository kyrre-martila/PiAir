//! Session timing owner shared by diagnostics and authenticated WAV playback.
//! It owns no media grant. The control owner remains responsible for TEARDOWN.
#[cfg(test)]
use super::Grandmaster;
use super::{Service, State};
use anyhow::{ensure, Result};
use std::{
    net::IpAddr,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Observing,
    PeerObserved,
    Failed,
    Closed,
}

/// One service, one epoch and one sample mapping. No clock replacement or retry.
pub struct TimingSession {
    service: Service,
    phase: Phase,
}
impl TimingSession {
    pub fn attach(service: Service) -> Self {
        Self {
            service,
            phase: Phase::Observing,
        }
    }
    pub fn evidence(&self) -> super::Evidence {
        self.service.evidence()
    }
    pub fn state(&self) -> State {
        self.service.state()
    }
    /// A current liveness check, not proof of synchronization or adoption.
    pub fn peer_is_live(&self, now: Instant) -> bool {
        self.state() == State::PeerObserved && self.service.clock().network_time(now).is_ok()
    }
    /// Local worker continuity and sends, separately from remote synchronization.
    pub fn healthy(&self, now: Instant) -> bool {
        let sent = self.evidence();
        matches!(self.phase, Phase::Observing | Phase::PeerObserved)
            && self.service.clock().network_time(now).is_ok()
            && sent.sync_sent > 0
            && sent.follow_up_sent > 0
            && sent.announce_sent > 0
            && sent.signaling_sent > 0
    }
    pub fn peer(&self) -> IpAddr {
        self.service.peer()
    }
    #[cfg(test)]
    #[cfg(test)]
    pub fn clock_mut(&mut self) -> &mut Grandmaster {
        self.service.clock_mut()
    }
    #[cfg(test)]
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn poll(&mut self, cancel: &AtomicBool) -> Result<()> {
        ensure!(
            matches!(self.phase, Phase::Observing | Phase::PeerObserved),
            "PTP session is terminal"
        );
        let result = self.service.poll(cancel);
        if result.is_err() {
            self.fail();
        } else if self.service.state() == State::PeerObserved {
            self.phase = Phase::PeerObserved;
        }
        result
    }
    /// Keep timing alive across bounded Wire requests. The scoped worker always
    /// joins before control can advance or resources can be dropped. Wire owns
    /// request deadlines and authorization; Service rechecks PtpLease per packet.
    pub fn during_control<T: Send>(
        &mut self,
        cancel: &AtomicBool,
        request: impl FnOnce() -> Result<T> + Send,
    ) -> Result<T> {
        self.poll(cancel)?;
        self.run_control(cancel, request)
    }
    fn run_control<T: Send>(
        &mut self,
        cancel: &AtomicBool,
        request: impl FnOnce() -> Result<T> + Send,
    ) -> Result<T> {
        let done = AtomicBool::new(false);
        let result = std::thread::scope(|scope| {
            let worker = scope.spawn(|| -> Result<()> {
                while !done.load(Ordering::Acquire) {
                    self.poll(cancel)?;
                    std::thread::park_timeout(Duration::from_millis(2));
                }
                self.poll(cancel)
            });
            let response = request();
            done.store(true, Ordering::Release);
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("PTP session worker panicked"))??;
            response
        });
        if result.is_err() {
            self.fail();
        }
        result
    }
    /// Timing loss must not suppress an otherwise authorized, bounded TEARDOWN.
    /// Wire still rechecks authorization; this never permits writes after revocation.
    pub fn during_cleanup<T: Send>(
        &mut self,
        cancel: &AtomicBool,
        request: impl FnOnce() -> Result<T> + Send,
    ) -> Result<T> {
        if self.poll(cancel).is_err() {
            return request();
        }
        self.run_control(cancel, request)
    }
    /// Scheduler integration: serialize RTP/PTP correspondence using only
    /// this owner's ClockID and epoch. Caller positions come from the existing
    /// 44.1 kHz RTP scheduler, never the legacy buffered 48 kHz anchor mapping.
    pub fn realtime_sync(
        &self,
        first: bool,
        current_rtp: u32,
        next_rtp: u32,
        now: Instant,
    ) -> Result<[u8; 28]> {
        ensure!(
            self.healthy(Instant::now()),
            "PTP timing service unavailable"
        );
        super::super::realtime::ptp_sync(
            first,
            current_rtp,
            next_rtp,
            self.service.clock().clock_id,
            self.service.clock().timestamp(now)?,
        )
    }
    /// Capture mapping from this service's identity and epoch, never caller IDs.
    /// This only constructs a paused diagnostic anchor; it cannot activate media.
    fn fail(&mut self) {
        self.service.shutdown();
        self.phase = Phase::Failed;
    }
    pub fn shutdown(&mut self) {
        self.service.shutdown();
        if self.phase != Phase::Failed {
            self.phase = Phase::Closed;
        }
    }
}
impl Drop for TimingSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}
