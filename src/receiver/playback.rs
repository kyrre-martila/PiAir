//! Shared bounded realtime WAV scheduler. Timing and transport stay session-owned.
use super::{
    profile::{RATE, SPF},
    realtime::{self, Encoder},
};
use anyhow::{ensure, Result};
use std::time::{Duration, Instant};

#[derive(Default, Debug)]
pub struct Statistics {
    pub packets: u64,
    pub bytes: u64,
    pub syncs: u64,
    pub requests: u64,
    pub resends: u64,
    pub elapsed: Duration,
}

pub trait Output {
    fn retransmissions_allowed(&self) -> bool {
        true
    }
    fn check(&mut self) -> Result<()>;
    fn feedback(&mut self) -> Result<()>;
    fn retransmit(&mut self) -> Result<Option<(u16, u16)>>;
    fn send(&mut self, packet: &[u8]) -> Result<()>;
    fn sync(&mut self, first: bool, current: u32, next: u32, at: Instant) -> Result<()>;
}

#[derive(Clone, Copy)]
pub(super) enum Policy {
    Wav,
    Live,
}
impl Policy {
    fn max_lateness(self) -> Duration {
        match self {
            Self::Wav => Duration::from_millis(100),
            // OwnTone player.c allows 1500 ms of read/write catch-up. This
            // remains below our existing two-second receiver playback lead.
            Self::Live => Duration::from_millis(1500),
        }
    }
}
struct Retries {
    remaining: usize,
    refill: Instant,
}
impl Retries {
    fn new(now: Instant) -> Self {
        Self {
            remaining: 1024,
            refill: now + Duration::from_secs(2),
        }
    }
    fn spend(&mut self, count: u16, now: Instant, policy: Policy) -> Result<()> {
        if matches!(policy, Policy::Live) && now >= self.refill {
            self.remaining = 1024;
            self.refill = now + Duration::from_secs(2);
        }
        ensure!((1..=32).contains(&count), "retransmission request limit");
        ensure!(
            self.remaining >= usize::from(count),
            "session retransmission budget exhausted"
        );
        self.remaining -= usize::from(count);
        Ok(())
    }
}

/// One absolute sample clock, including the existing two-second silent drain.
/// A partial WAV block is padded, never encoded as a second ALAC packet format.
pub fn run(
    output: &mut impl Output,
    encoder: &mut Encoder,
    frames: &[[i16; 2]],
    stats: &mut Statistics,
) -> Result<()> {
    ensure!(
        !frames.is_empty() && frames.len() <= 49 * RATE as usize,
        "WAV input must contain 1..=2160900 frames"
    );
    let start = Instant::now();
    let result = run_at(
        output,
        encoder,
        frames,
        stats,
        start,
        Instant::now,
        std::thread::park_timeout,
    );
    stats.elapsed = start.elapsed();
    encoder.flush();
    result
}

fn run_at(
    output: &mut impl Output,
    encoder: &mut Encoder,
    frames: &[[i16; 2]],
    stats: &mut Statistics,
    start: Instant,
    mut now: impl FnMut() -> Instant,
    mut wait: impl FnMut(Duration),
) -> Result<()> {
    run_source(
        output,
        encoder,
        stats,
        start,
        &mut now,
        &mut wait,
        Some(frames.len() as u64 + u64::from(realtime::PLAYBACK_LEAD_SAMPLES)),
        Policy::Wav,
        |sent| {
            Ok(std::array::from_fn(|n| {
                frames.get(sent as usize + n).copied().unwrap_or([0; 2])
            }))
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_source(
    output: &mut impl Output,
    encoder: &mut Encoder,
    stats: &mut Statistics,
    start: Instant,
    mut now: impl FnMut() -> Instant,
    mut wait: impl FnMut(Duration),
    total: Option<u64>,
    policy: Policy,
    mut source: impl FnMut(u64) -> Result<[[i16; 2]; SPF]>,
) -> Result<()> {
    run_optional_source(
        output,
        encoder,
        stats,
        start,
        &mut now,
        &mut wait,
        total,
        policy,
        |sent| source(sent).map(Some),
    )
}

/// EOF drains silence on the same sample clock; no RTP/PTP reset at auto-stop.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_optional_source(
    output: &mut impl Output,
    encoder: &mut Encoder,
    stats: &mut Statistics,
    start: Instant,
    mut now: impl FnMut() -> Instant,
    mut wait: impl FnMut(Duration),
    mut total: Option<u64>,
    policy: Policy,
    mut source: impl FnMut(u64) -> Result<Option<[[i16; 2]; SPF]>>,
) -> Result<()> {
    let mut ended = false;
    let mut sent = 0;
    let mut sync_at = 0;
    let mut feedback_at = start + Duration::from_secs(2);
    let mut retries = Retries::new(start);
    let mut max_lateness_us = 0u64;
    tracing::debug!(target: "piair::rtsp", phase = "playback policy", feedback_polling = true,
        retransmissions_allowed = output.retransmissions_allowed(), total_frames = total,
        "diagnostic policy; UDP sends do not establish receiver acceptance");
    while total.is_none_or(|total| sent < total) {
        output.check()?;
        let deadline = start + realtime::sample_time(sent);
        ensure!(
            now().saturating_duration_since(deadline) < policy.max_lateness(),
            "audio scheduler stalled; no uncontrolled catch-up"
        );
        if now() >= feedback_at {
            output.feedback()?;
            feedback_at += Duration::from_secs(2);
        }
        if let Some((first, count)) = output.retransmit()? {
            ensure!((1..=32).contains(&count), "retransmission request limit");
            stats.requests += 1;
            tracing::debug!(target: "piair::rtsp", phase = "receiver control", first_sequence = first, requested_packets = count,
                allowed = output.retransmissions_allowed(), "receiver requested retransmission");
            ensure!(
                output.retransmissions_allowed(),
                "playback check forbids retransmissions"
            );
            retries.spend(count, now(), policy)?;
            for offset in 0..count {
                if let Some(packet) = encoder.resend(first.wrapping_add(offset))? {
                    output.check()?;
                    output.send(packet)?;
                    stats.resends += 1;
                }
            }
        }
        if let Some(remaining) = deadline
            .checked_duration_since(now())
            .filter(|d| !d.is_zero())
        {
            wait(remaining.min(Duration::from_millis(5)));
            continue;
        }
        ensure!(
            now().saturating_duration_since(deadline) < policy.max_lateness(),
            "control feedback stalled the sample timeline"
        );
        max_lateness_us =
            max_lateness_us.max(now().saturating_duration_since(deadline).as_micros() as u64);
        if sent >= sync_at {
            let next = encoder.timestamp;
            output.check()?;
            output.sync(
                sent == 0,
                next.wrapping_sub(realtime::SYNC_OFFSET_SAMPLES),
                next,
                deadline,
            )?;
            stats.syncs += 1;
            tracing::debug!(target: "piair::rtsp", phase = "sync", initial = sent == 0,
                sync_packets = stats.syncs, sent_frames = sent, next_timestamp = next, max_lateness_us,
                "sync datagram sent; receiver scheduling unverified");
            sync_at = sent + u64::from(RATE);
        }
        let pcm = if ended {
            [[0; 2]; SPF]
        } else {
            match source(sent)? {
                Some(pcm) => pcm,
                None => {
                    ended = true;
                    total = Some(sent + u64::from(realtime::PLAYBACK_LEAD_SAMPLES));
                    [[0; 2]; SPF]
                }
            }
        };
        let packet = encoder.packet(&pcm)?;
        output.check()?;
        output.send(&packet)?;
        if sent == 0 || total.is_some_and(|total| sent + SPF as u64 >= total) {
            tracing::debug!(target: "piair::rtsp", phase = "RTP boundary", datagram_bytes = packet.len(),
                "media datagram sent; receiver acceptance unverified");
        }
        stats.packets += 1;
        stats.bytes += packet.len() as u64;
        sent += SPF as u64;
    }
    let drained = start + realtime::sample_time(total.unwrap_or(sent));
    while let Some(remaining) = drained
        .checked_duration_since(now())
        .filter(|d| !d.is_zero())
    {
        output.check()?;
        wait(remaining.min(Duration::from_millis(5)));
    }
    output.check()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::VecDeque, rc::Rc};
    struct Capture {
        now: Rc<Cell<Instant>>,
        start: Instant,
        packets: Vec<(Duration, Vec<u8>)>,
        syncs: Vec<(bool, u32, u32, Duration)>,
        requests: VecDeque<(u16, u16)>,
        fail_after: Option<usize>,
        feedback: usize,
    }
    impl Output for Capture {
        fn check(&mut self) -> Result<()> {
            ensure!(
                self.fail_after.is_none_or(|n| self.packets.len() < n),
                "injected cancellation/timing loss"
            );
            Ok(())
        }
        fn feedback(&mut self) -> Result<()> {
            self.feedback += 1;
            Ok(())
        }
        fn retransmit(&mut self) -> Result<Option<(u16, u16)>> {
            Ok(self.requests.pop_front())
        }
        fn send(&mut self, packet: &[u8]) -> Result<()> {
            self.packets
                .push((self.now.get() - self.start, packet.to_vec()));
            Ok(())
        }
        fn sync(&mut self, first: bool, current: u32, next: u32, at: Instant) -> Result<()> {
            self.syncs.push((first, current, next, at - self.start));
            Ok(())
        }
    }
    fn capture() -> Capture {
        let start = Instant::now();
        Capture {
            now: Rc::new(Cell::new(start)),
            start,
            packets: vec![],
            syncs: vec![],
            requests: VecDeque::new(),
            fail_after: None,
            feedback: 0,
        }
    }
    #[test]
    fn absolute_pacing_initial_periodic_sync_partial_eof_and_bounded_drain() {
        let mut output = capture();
        let now = output.now.clone();
        let wait = now.clone();
        let mut encoder = Encoder::new().unwrap();
        let origin = encoder.timestamp;
        let mut stats = Statistics::default();
        run_at(
            &mut output,
            &mut encoder,
            &[[20, -40]; 353],
            &mut stats,
            now.get(),
            || now.get(),
            |d| wait.set(wait.get() + d),
        )
        .unwrap();
        assert_eq!(stats.packets, 252);
        assert_eq!(stats.syncs, 2);
        assert_eq!(output.feedback, 1);
        assert_eq!(now.get() - output.start, realtime::sample_time(88553));
        for (index, (at, packet)) in output.packets.iter().enumerate() {
            assert_eq!(*at, realtime::sample_time((index * SPF) as u64));
            assert_eq!(
                u32::from_be_bytes(packet[4..8].try_into().unwrap()),
                origin.wrapping_add((index * SPF) as u32)
            );
        }
        for (index, (first, current, next, at)) in output.syncs.iter().enumerate() {
            let sent = index as u64 * u64::from(RATE).div_ceil(SPF as u64) * SPF as u64;
            assert_eq!(*first, index == 0);
            assert_eq!(*at, realtime::sample_time(sent));
            assert_eq!(*next, origin.wrapping_add(sent as u32));
            assert_eq!(*current, next.wrapping_sub(77175));
        }
    }
    #[test]
    fn cancellation_timing_loss_malformed_requests_and_stalls_stop_sends() {
        for mode in 0..4 {
            let mut output = capture();
            let now = output.now.clone();
            let wait = now.clone();
            let mut encoder = Encoder::new().unwrap();
            let mut stats = Statistics::default();
            if mode == 0 {
                output.fail_after = Some(0);
            }
            if mode == 1 {
                output.fail_after = Some(2);
            }
            if mode == 2 {
                output.requests.push_back((encoder.sequence, 33));
            }
            let result = run_at(
                &mut output,
                &mut encoder,
                &[[0; 2]; 353],
                &mut stats,
                now.get(),
                || now.get(),
                |d| wait.set(wait.get() + if mode == 3 { Duration::from_secs(1) } else { d }),
            );
            assert!(result.is_err());
            assert!(stats.packets <= 2);
            assert_eq!(output.packets.len() as u64, stats.packets);
        }
    }
    #[test]
    fn automatic_eof_drains_on_existing_timeline_and_keeps_feedback_alive() {
        let mut output = capture();
        let now = output.now.clone();
        let wait = now.clone();
        let mut encoder = Encoder::new().unwrap();
        let origin = encoder.timestamp;
        let mut stats = Statistics::default();
        let mut calls = 0;
        run_optional_source(
            &mut output,
            &mut encoder,
            &mut stats,
            now.get(),
            || now.get(),
            |d| wait.set(wait.get() + d),
            None,
            Policy::Live,
            |_| {
                calls += 1;
                Ok((calls <= 3).then_some([[100; 2]; SPF]))
            },
        )
        .unwrap();
        assert_eq!(calls, 4); // EOF is latched, never reads new record into old session.
        assert!(stats.packets >= 253);
        assert!(output.feedback > 0);
        for (i, (_, packet)) in output.packets.iter().enumerate() {
            assert_eq!(
                u32::from_be_bytes(packet[4..8].try_into().unwrap()),
                origin.wrapping_add((i * SPF) as u32)
            );
        }
        assert!(now.get() - output.start >= Duration::from_secs(2));
    }
    #[test]
    fn live_underrun_keeps_timestamps_and_source_failure_stops_sends() {
        for fail in [false, true] {
            let mut output = capture();
            let now = output.now.clone();
            let wait = now.clone();
            let mut encoder = Encoder::new().unwrap();
            encoder.timestamp = u32::MAX - 100;
            let origin = encoder.timestamp;
            let mut stats = Statistics::default();
            let mut reads = 0;
            let result = run_source(
                &mut output,
                &mut encoder,
                &mut stats,
                now.get(),
                || now.get(),
                |d| wait.set(wait.get() + d),
                Some(4 * SPF as u64),
                Policy::Live,
                |sent| {
                    reads += 1;
                    if fail && reads == 3 {
                        anyhow::bail!("capture disconnected");
                    }
                    Ok(if sent == SPF as u64 {
                        [[0; 2]; SPF]
                    } else {
                        [[123, -456]; SPF]
                    })
                },
            );
            assert_eq!(result.is_err(), fail);
            assert_eq!(stats.packets, if fail { 2 } else { 4 });
            for (i, (_, packet)) in output.packets.iter().enumerate() {
                assert_eq!(
                    u32::from_be_bytes(packet[4..8].try_into().unwrap()),
                    origin.wrapping_add((i * SPF) as u32)
                );
            }
        }
    }
    #[test]
    fn indefinite_source_obeys_network_termination() {
        let mut output = capture();
        output.fail_after = Some(3);
        let now = output.now.clone();
        let wait = now.clone();
        let mut encoder = Encoder::new().unwrap();
        let mut stats = Statistics::default();
        assert!(run_source(
            &mut output,
            &mut encoder,
            &mut stats,
            now.get(),
            || now.get(),
            |d| wait.set(wait.get() + d),
            None,
            Policy::Live,
            |_| Ok([[0; 2]; SPF])
        )
        .is_err());
        assert_eq!(stats.packets, 3);
    }
    #[test]
    fn live_retransmission_budget_renews_for_hours_but_floods_still_stop() {
        let start = Instant::now();
        let mut live = Retries::new(start);
        let mut wav = Retries::new(start);
        for second in 0..6 * 3600 {
            live.spend(32, start + Duration::from_secs(second), Policy::Live)
                .unwrap();
        }
        for _ in 0..32 {
            wav.spend(32, start, Policy::Wav).unwrap();
        }
        assert!(wav
            .spend(1, start + Duration::from_secs(3600), Policy::Wav)
            .is_err());
        let mut live = Retries::new(start);
        for _ in 0..32 {
            live.spend(32, start, Policy::Live).unwrap();
        }
        assert!(live
            .spend(1, start + Duration::from_millis(1999), Policy::Live)
            .is_err());
        live.spend(32, start + Duration::from_secs(2), Policy::Live)
            .unwrap();
        assert!(live
            .spend(33, start + Duration::from_secs(4), Policy::Live)
            .is_err());
    }
    #[test]
    fn live_scheduler_tolerates_sub_lead_delay_without_resetting_timestamps() {
        for (policy, pause, fails) in [
            (Policy::Wav, 200, true),
            (Policy::Live, 200, false),
            (Policy::Live, 1600, true),
        ] {
            let mut output = capture();
            let now = output.now.clone();
            let wait = now.clone();
            let mut paused = false;
            let mut encoder = Encoder::new().unwrap();
            let origin = encoder.timestamp;
            let mut stats = Statistics::default();
            let result = run_source(
                &mut output,
                &mut encoder,
                &mut stats,
                now.get(),
                || now.get(),
                |d| {
                    wait.set(
                        wait.get()
                            + d
                            + if paused {
                                Duration::ZERO
                            } else {
                                paused = true;
                                Duration::from_millis(pause)
                            },
                    );
                },
                Some(30 * SPF as u64),
                policy,
                |_| Ok([[123, -456]; SPF]),
            );
            assert_eq!(result.is_err(), fails, "pause={pause} ms");
            for (i, (_, packet)) in output.packets.iter().enumerate() {
                assert_eq!(
                    u32::from_be_bytes(packet[4..8].try_into().unwrap()),
                    origin.wrapping_add((i * SPF) as u32)
                );
            }
        }
    }
    #[test]
    fn missing_packet_requests_are_bounded_without_reencryption() {
        let mut output = capture();
        output.requests.push_back((5, 1));
        let now = output.now.clone();
        let wait = now.clone();
        let mut encoder = Encoder::new().unwrap();
        encoder.sequence = 10;
        let mut stats = Statistics::default();
        run_at(
            &mut output,
            &mut encoder,
            &[[0; 2]; 1],
            &mut stats,
            now.get(),
            || now.get(),
            |d| wait.set(wait.get() + d),
        )
        .unwrap();
        assert_eq!(stats.requests, 1);
        assert_eq!(stats.resends, 0);
        assert_eq!(stats.packets, 251);
    }
}
