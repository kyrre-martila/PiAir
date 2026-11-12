//! Session timing owner shared by diagnostics and future authenticated WAV setup.
//! It owns no media grant. The control owner remains responsible for TEARDOWN.
use super::{
    control::{AnchorRequest, Rate},
    time::SampleTimeline,
    Grandmaster, Service, State,
};
use anyhow::{ensure, Context, Result};
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
    timeline: Option<SampleTimeline>,
    phase: Phase,
}
impl TimingSession {
    pub fn attach(service: Service) -> Self {
        Self {
            service,
            timeline: None,
            phase: Phase::Observing,
        }
    }
    pub fn state(&self) -> State {
        self.service.state()
    }
    pub fn peer(&self) -> IpAddr {
        self.service.peer()
    }
    #[cfg(test)]
    pub fn clock(&self) -> &Grandmaster {
        self.service.clock()
    }
    #[cfg(test)]
    pub fn clock_mut(&mut self) -> &mut Grandmaster {
        self.service.clock_mut()
    }
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
    /// Capture mapping from this service's identity and epoch, never caller IDs.
    /// This only constructs a paused diagnostic anchor; it cannot activate media.
    pub fn paused_anchor(&mut self, sample: u64, now: Instant) -> Result<AnchorRequest> {
        let result = (|| {
            ensure!(self.phase == Phase::PeerObserved, "PTP peer not observed");
            ensure!(self.timeline.is_none(), "PTP timeline already bound");
            let clock = self.service.clock_mut();
            let mapping = mapping(clock, sample, now)?;
            let anchor = AnchorRequest::generate(clock, &mapping, sample, Rate::Pause, now)?;
            self.timeline = Some(mapping);
            Ok(anchor)
        })();
        if result.is_err() {
            self.fail();
        }
        result
    }
    pub fn validate_anchor(&mut self, anchor: &AnchorRequest, now: Instant) -> Result<()> {
        let result = (|| {
            ensure!(self.timeline.is_some(), "PTP timeline absent");
            self.service.clock_mut().check(now, false)?;
            anchor.validate(self.service.clock(), now)
        })();
        if result.is_err() {
            self.fail();
        }
        result
    }
    fn fail(&mut self) {
        self.service.shutdown();
        self.timeline = None;
        self.phase = Phase::Failed;
    }
    pub fn shutdown(&mut self) {
        self.service.shutdown();
        self.timeline = None;
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

fn mapping(clock: &mut Grandmaster, sample: u64, now: Instant) -> Result<SampleTimeline> {
    clock.check(now, false)?;
    ensure!(
        clock.state() == State::PeerObserved,
        "no valid receiver timing evidence"
    );
    let origin = clock.network_time(
        now.checked_add(Duration::from_secs(1))
            .context("PTP anchor deadline overflow")?,
    )?;
    SampleTimeline::new(clock.clock_id, clock.generation(), origin, sample)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receiver::ptp::packet::{Kind, Packet};
    #[test]
    fn mapping_uses_owned_clock_epoch_and_generation_without_media_permission() {
        let now = Instant::now();
        let mut clock = Grandmaster::new(11, 22, now, Duration::from_secs(2)).unwrap();
        assert!(mapping(&mut clock, 0, now).is_err());
        let wrong = Packet::new(Kind::DelayReq, 33, 0, vec![0; 10])
            .encode()
            .unwrap();
        assert!(clock.receive(&wrong, now).is_err());
        let request = Packet::new(Kind::DelayReq, 22, 0, vec![0; 10])
            .encode()
            .unwrap();
        clock.receive(&request, now).unwrap();
        let timeline = mapping(&mut clock, u32::MAX as u64, now).unwrap();
        assert_eq!(timeline.clock_id, 11);
        assert_eq!(
            timeline.at(u32::MAX as u64).unwrap(),
            clock.network_time(now + Duration::from_secs(1)).unwrap()
        );
        assert_eq!(timeline.at(u32::MAX as u64 + 48_000).unwrap().seconds, 2);
        let anchor =
            AnchorRequest::generate(&mut clock, &timeline, u32::MAX as u64, Rate::Pause, now)
                .unwrap();
        let mut replacement = Grandmaster::new(11, 22, now, Duration::from_secs(2)).unwrap();
        replacement.receive(&request, now).unwrap();
        assert!(anchor.validate(&replacement, now).is_err());
        assert!(!clock.media_permitted());
        clock.check(now, true).unwrap_err();
        assert!(anchor.validate(&clock, now).is_err());
        assert!(mapping(&mut clock, 0, now).is_err());
        assert_eq!(clock.state(), State::Shutdown);
    }
    #[test]
    fn absolute_setup_timeout_and_liveness_expiry_invalidate_mapping() {
        let now = Instant::now();
        let mut clock = Grandmaster::new(11, 22, now, Duration::from_millis(100)).unwrap();
        clock.check(now, false).unwrap();
        assert!(clock
            .check(now + Duration::from_millis(100), false)
            .is_err());
        assert!(mapping(&mut clock, 0, now + Duration::from_millis(100)).is_err());
        assert_eq!(clock.state(), State::Lost);
    }
}
