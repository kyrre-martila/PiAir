//! Single authenticated receiver owner for PTP realtime and legacy buffered diagnostics.
//! No public way to assert receiver clock adoption exists. See the evidence ledger.
use super::playback::Output as _;
use super::{
    buffered::{NegotiatedStream, Prepared},
    pairing,
    planning::{Claim, Evidence},
    profile, ptp, realtime,
    store::Credential,
    wire::{Wire, Worker},
};
use anyhow::{bail, ensure, Context, Result};
use plist::Value;
use std::{
    net::IpAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub const PLAYBACK_BLOCKER: &str = "buffered playback unavailable: receiver clock adoption is unverified; PeerObserved, elapsed time and RTSP 200 never authorize media";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Authorization,
    Authentication,
    Negotiation,
    Timing,
    // Buffered playback retains its separate adoption gate.
    #[allow(dead_code)]
    Prebuffering,
    Playing,
    #[allow(dead_code)]
    Paused,
    Stopping,
    Failed,
    Closed,
}
impl State {
    fn advance(&mut self, next: Self) -> Result<()> {
        ensure!(
            matches!(
                (*self, next),
                (Self::Authorization, Self::Authentication)
                    | (Self::Authentication, Self::Negotiation)
                    | (Self::Negotiation, Self::Timing)
                    | (Self::Timing, Self::Playing)
                    | (
                        Self::Authorization
                            | Self::Authentication
                            | Self::Negotiation
                            | Self::Timing
                            | Self::Playing,
                        Self::Stopping
                    )
                    | (Self::Stopping, Self::Closed | Self::Failed)
            ),
            "illegal buffered session transition {self:?} -> {next:?}"
        );
        *self = next;
        Ok(())
    }
}

/// Explicit diagnostic progression. An accepted stage is never media readiness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub enum Stage {
    Record,
    Stream,
    Anchor,
}

type LiveInput<'a> = (
    Box<dyn FnOnce() -> Result<crate::audio::live::Capture> + 'a>,
    Option<u32>,
);

/// Owns authenticated control, timing and event resources, plus a fresh stream
/// key after negotiation. Scoped timing workers cannot outlive control calls.
/// Realtime media stays owned here; no reusable playback grant escapes.
pub struct Session {
    state: State,
    last_accepted: &'static str,
    rtsp_stages: Vec<String>,
    attempted: &'static str,
    final_ptp: Option<ptp::State>,
    final_evidence: ptp::Evidence,
    teardown: &'static str,
    control: Option<Wire>,
    timing: Option<ptp::session::TimingSession>,
    events: Option<Worker>,
    uri: String,
    token: Option<String>,
    // Initial SETUP fixes token presence for this owner and its exclusive Wire.
    session_bound: bool,
    attempted_setup: bool,
    setup_rtsp_accepted: bool,
    observed: bool,
    observation_complete: bool,
    // Historical result at observation completion (or before cleanup on failure).
    // Never recomputed from Shutdown and never used to authorize media.
    observed_peer_liveness: Option<bool>,
    stage: Option<Stage>,
    prepared: Option<Prepared>,
    stream: Option<NegotiatedStream>,
    anchor_accepted: bool,
    realtime_profile: bool,
    realtime_transport: Option<super::wire::RealtimeTransport>,
    realtime_encoder: Option<realtime::Encoder>,
    realtime_ports: Option<(u16, u16)>,
    realtime_stream_id: Option<u32>,
    playback_stats: super::playback::Statistics,
    playback_outcome: &'static str,
    realtime_ready_snapshot: bool,
    timing_health_snapshot: bool,
    clock_id: u64,
    peer_clock_id: Option<u64>,
    uuid: uuid::Uuid,
    pub cancel: Arc<AtomicBool>,
}
impl Session {
    pub fn new() -> Self {
        let clock_id = loop {
            let id = rand::random();
            if id != 0 && id != u64::MAX {
                break id;
            }
        };
        Self {
            state: State::Authorization,
            last_accepted: "none",
            rtsp_stages: Vec::new(),
            attempted: "none",
            final_ptp: None,
            final_evidence: ptp::Evidence::default(),
            teardown: "not attempted",
            control: None,
            timing: None,
            events: None,
            uri: String::new(),
            token: None,
            session_bound: false,
            attempted_setup: false,
            setup_rtsp_accepted: false,
            observed: false,
            observation_complete: false,
            observed_peer_liveness: None,
            stage: None,
            prepared: None,
            stream: None,
            anchor_accepted: false,
            realtime_profile: false,
            realtime_transport: None,
            realtime_encoder: None,
            realtime_ports: None,
            realtime_stream_id: None,
            playback_stats: super::playback::Statistics::default(),
            playback_outcome: "not authorized",
            realtime_ready_snapshot: false,
            timing_health_snapshot: false,
            clock_id,
            peer_clock_id: None,
            uuid: uuid::Uuid::new_v4(),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn state(&self) -> State {
        self.state
    }
    /// Fixed vocabulary only: never includes addresses, tokens, keys or payloads.
    /// PTP is a snapshot before cleanup, not a continuing lock assertion.
    pub fn diagnostic(&self) -> String {
        let ptp = self
            .timing
            .as_ref()
            .map(ptp::session::TimingSession::state)
            .or(self.final_ptp);
        let evidence = self
            .timing
            .as_ref()
            .map(|s| s.evidence())
            .unwrap_or(self.final_evidence);
        let peer_live = self.observed_peer_liveness.unwrap_or_else(|| {
            self.timing
                .as_ref()
                .is_some_and(|timing| timing.peer_is_live(Instant::now()))
        });
        let observation = if self.observation_complete {
            "completed"
        } else {
            "incomplete"
        };
        let local_cleanup = if matches!(self.state, State::Closed | State::Failed)
            && self.timing.is_none()
            && self.control.is_none()
        {
            "released"
        } else {
            "pending"
        };
        let realtime = if self.realtime_profile {
            format!("stream type: 96; negotiated ports: {:?}; realtime media transport negotiated: {}; operational readiness at snapshot: {}; timing healthy at snapshot: {}; clock identity: {:016x}; authenticated peer clock identity: {:?}; peer-role validation: pinned authenticated ClockID, per-source identity sequences, no clock election; playback outcome: {}; audio packets: {}; audio bytes: {}; retransmission requests: {}; retransmission responses: {}; RTP/PTP sync packets: {}; playback elapsed: {:?}; audible playback: unverified; ",
                self.realtime_ports, self.realtime_ports.is_some(), self.realtime_ready_snapshot,
                self.timing_health_snapshot, self.clock_id, self.peer_clock_id.map(|id| format!("{id:016x}")),
                self.playback_outcome, self.playback_stats.packets, self.playback_stats.bytes, self.playback_stats.requests, self.playback_stats.resends, self.playback_stats.syncs, self.playback_stats.elapsed)
        } else {
            String::new()
        };
        format!("{realtime}local cleanup: {local_cleanup}; negotiation observation: {observation}; peer liveness at snapshot: {peer_live}; synchronization: unverified; initial SETUP RTSP accepted: {}; RTSP stages: {:?}; packet evidence: {evidence:?}; media readiness: requires active session authorization; session: {:?}; last accepted: {}; attempted: {}; PTP snapshot: {ptp:?}; receiver clock adoption: unverified; TEARDOWN: {}; audio bytes: {}",
            self.setup_rtsp_accepted, self.rtsp_stages, self.state(), self.last_accepted, self.attempted, self.teardown, self.playback_stats.bytes)
    }
    fn check(&self) -> Result<()> {
        ensure!(
            !self.cancel.load(Ordering::Relaxed),
            "buffered diagnostic cancelled"
        );
        self.control
            .as_ref()
            .context("control absent")?
            .ptp_lease()?
            .check()?;
        if let Some(events) = &self.events {
            events.check()?;
        }
        Ok(())
    }
    /// Explicit initial-SETUP diagnostic, not a playback or full stream check.
    /// The factory must use Wire::connect with an already selected receiver.
    pub fn start(
        &mut self,
        connect: impl FnOnce() -> Result<Wire>,
        credential: Option<&Credential>,
    ) -> Result<()> {
        ensure!(
            self.state == State::Authorization,
            "buffered session already used"
        );
        let result = (|| {
            ensure!(
                !self.cancel.load(Ordering::Relaxed),
                "buffered diagnostic cancelled"
            );
            let mut wire = connect()?;
            wire.buffered_authorization()?;
            wire.diagnostic_cancel = Some(self.cancel.clone());
            self.state.advance(State::Authentication)?;
            self.control = Some(
                if wire.method() == super::compatibility::Method::Transient {
                    ensure!(
                        credential.is_none(),
                        "transient cannot consume persistent credentials"
                    );
                    wire.buffered_transient()?;
                    pairing::transient(wire)?
                } else {
                    pairing::verify(
                        wire,
                        credential.context("stored credentials required; no automatic pairing")?,
                    )?
                },
            );
            self.state.advance(State::Negotiation)?;
            self.negotiate()
        })();
        self.finish(result)
    }
    /// Transfer the already authenticated control channel from the WAV session
    /// foundation. No authentication retry, socket redial or NTP fallback occurs.
    pub(super) fn from_authenticated(wire: Wire, cancel: Arc<AtomicBool>) -> Result<Self> {
        let mut owner = Self::new();
        owner.cancel = cancel;
        let mut wire = wire;
        wire.diagnostic_cancel = Some(owner.cancel.clone());
        owner.control = Some(wire);
        owner.state.advance(State::Authentication)?;
        let result = (|| {
            owner.check()?;
            owner
                .control
                .as_ref()
                .context("control absent")?
                .buffered_authorization()?;
            owner.state.advance(State::Negotiation)
        })();
        owner.finish(result)?;
        Ok(owner)
    }
    /// Initial PTP SETUP only. The transferred owner is deliberately inert until
    /// this explicit call; normal WAV negotiation never enters it automatically.
    pub fn setup_authenticated(&mut self) -> Result<()> {
        ensure!(
            self.state == State::Negotiation && !self.attempted_setup,
            "PTP setup already used or unavailable"
        );
        let result = self.negotiate();
        self.finish(result)
    }
    /// Explicit caller authorization for the complete no-audio realtime check.
    /// Must be selected before /info so buffered capabilities cannot gate type 96.
    pub fn setup_realtime_authenticated(&mut self) -> Result<()> {
        ensure!(
            self.state == State::Negotiation && !self.attempted_setup,
            "PTP setup already used or unavailable"
        );
        self.realtime_profile = true;
        self.setup_authenticated()
    }
    /// OwnTone's realtime startup, with volume omitted by explicit user policy.
    /// No buffered command registration, audioMode POST or anchor is involved.
    pub fn negotiate_realtime(&mut self) -> Result<()> {
        let result = (|| {
            ensure!(
                self.realtime_profile,
                "realtime profile not explicitly authorized"
            );
            self.progress_inner(Stage::Record)?;
            self.set_timing_peers()?;
            let transport = self
                .control
                .as_ref()
                .context("control absent")?
                .prepare_ptp_realtime()?;
            let encoder = realtime::Encoder::from_verified_key(
                self.control
                    .as_mut()
                    .context("control absent")?
                    .take_audio_key()?,
            )?;
            let connection = rand::random::<u32>();
            let offer =
                profile::stream_offer(encoder.key(), transport.local_control()?, connection)?;
            // Install resources before the request so every failure erases the key.
            self.realtime_transport = Some(transport);
            self.realtime_encoder = Some(encoder);
            let reply = self.call("SETUP", None, &offer)?;
            let response = Self::plist(&reply)?;
            ptp::control::acknowledgement(&reply.body, Some(&response))?;
            let stream = profile::stream_for_timing(&response, connection, "PTP")?;
            self.realtime_transport
                .as_mut()
                .context("realtime transport absent")?
                .connect(&stream)?;
            self.realtime_ports = Some((stream.data, stream.control));
            self.realtime_stream_id = stream.id;
            self.last_accepted = "realtime stream SETUP";
            self.stage = Some(Stage::Stream);
            self.check()?;
            Ok(())
        })();
        self.finish(result)
    }
    /// Operational readiness is not evidence of receiver synchronization. The
    /// session owner must recheck this at every media operation in PR 63.
    pub fn realtime_ready(&self) -> Result<()> {
        self.check()?;
        ensure!(
            self.realtime_profile
                && matches!(self.state, State::Timing | State::Playing)
                && self.stage == Some(Stage::Stream)
                && self.session_bound
                && self.realtime_encoder.is_some(),
            "realtime negotiation incomplete"
        );
        self.realtime_transport
            .as_ref()
            .context("realtime transport absent")?
            .check()?;
        let timing = self.timing.as_ref().context("timing absent")?;
        ensure!(timing.peer_is_live(Instant::now()), "PTP peer not live");
        ensure!(timing.healthy(Instant::now()), "timing service not healthy");
        Ok(())
    }
    /// Explicit playback authorization on this negotiated owner. No redial, key
    /// replacement, NTP fallback or buffered anchor is possible here.
    pub fn play_wav(&mut self, frames: &[[i16; 2]], volume_db: Option<i32>) -> Result<()> {
        self.play_pcm(Some(frames), None, volume_db)
    }
    pub fn play_live(
        &mut self,
        device: &str,
        seconds: Option<u32>,
        volume: Option<i32>,
    ) -> Result<()> {
        self.play_pcm(
            None,
            Some((
                Box::new(move || crate::audio::live::Capture::start(device)),
                seconds,
            )),
            volume,
        )
    }
    pub fn play_automatic(
        &mut self,
        capture: crate::audio::live::Capture,
        volume: Option<i32>,
    ) -> Result<()> {
        self.play_pcm(None, Some((Box::new(move || Ok(capture)), None)), volume)
    }
    fn play_pcm(
        &mut self,
        frames: Option<&[[i16; 2]]>,
        live: Option<LiveInput<'_>>,
        volume_db: Option<i32>,
    ) -> Result<()> {
        let result = (|| {
            ensure!(
                self.playback_outcome == "not authorized",
                "playback already used"
            );
            ensure!(
                frames.is_none_or(
                    |frames| !frames.is_empty() && frames.len() <= 30 * profile::RATE as usize
                ),
                "WAV duration outside bound"
            );
            if let Some(volume) = volume_db {
                ensure!((-60..=-10).contains(&volume), "volume must be -60..=-10 dB");
            }
            // Wait for the established timing service's operational readiness.
            // This is peer liveness, never a cryptographic clock-adoption proof.
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                self.check()?;
                self.timing
                    .as_mut()
                    .context("timing absent")?
                    .poll(&self.cancel)?;
                if self.realtime_ready().is_ok() {
                    break;
                }
                ensure!(Instant::now() < deadline, "PTP media readiness timed out");
                std::thread::park_timeout(Duration::from_millis(2));
            }
            self.realtime_ready_snapshot = true;
            if let Some(volume) = volume_db {
                let body = format!("volume: {volume}.000000\r\n");
                self.set_volume(body.as_bytes())?;
            }
            self.realtime_ready()?;
            self.state.advance(State::Playing)?;
            self.playback_outcome = "transmitting";
            self.transmit_pcm_owned(frames, live)
        })();
        self.playback_outcome = if result.is_ok() {
            "local transmission completed"
        } else {
            "failed"
        };
        self.finish(result)
    }
    fn set_volume(&mut self, body: &[u8]) -> Result<()> {
        self.check()?;
        let wire = self.control.as_mut().context("control absent")?;
        let headers: Vec<_> = std::iter::once(("Content-Type", "text/parameters".into()))
            .chain(self.token.iter().map(|token| ("Session", token.clone())))
            .collect();
        let reply = self
            .timing
            .as_mut()
            .context("timing absent")?
            .during_control(&self.cancel, || {
                wire.request("SET_PARAMETER", &self.uri, &headers, body)
            })?;
        if let Some(token) = reply.headers.get("session") {
            ensure!(
                self.token.as_ref() == Some(token),
                "volume response changed session binding"
            );
        }
        ensure!(reply.body.is_empty(), "unexpected volume response body");
        self.rtsp_stages
            .push("SET_PARAMETER volume: RTSP accepted".into());
        Ok(())
    }
    fn transmit_pcm_owned(
        &mut self,
        frames: Option<&[[i16; 2]]>,
        live: Option<LiveInput<'_>>,
    ) -> Result<()> {
        let limit = live
            .as_ref()
            .and_then(|(_, seconds)| *seconds)
            .map(|s| u64::from(s) * u64::from(profile::RATE));
        let mut capture = live.map(|(open, _)| open()).transpose()?;
        let lease = self
            .control
            .as_ref()
            .context("control absent")?
            .ptp_lease()?;
        let mut feedback = super::session::FeedbackWorker::start_bound(
            self.control.take().context("control absent")?,
            self.token.clone(),
            self.realtime_stream_id,
            self.cancel.clone(),
            true,
        )?;
        let mut encoder = self.realtime_encoder.take().context("encoder absent")?;
        let timing = std::sync::Mutex::new(self.timing.take().context("timing absent")?);
        let done = AtomicBool::new(false);
        let mut result = std::thread::scope(|scope| {
            // A drop guard also stops the worker if the media thread unwinds.
            struct Stop<'a>(&'a AtomicBool);
            impl Drop for Stop<'_> {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::Release);
                }
            }
            let worker = scope.spawn(|| -> Result<()> {
                while !done.load(Ordering::Acquire) {
                    timing
                        .lock()
                        .map_err(|_| anyhow::anyhow!("PTP worker poisoned"))?
                        .poll(&self.cancel)?;
                    std::thread::park_timeout(Duration::from_millis(2));
                }
                Ok(())
            });
            let stop = Stop(&done);
            let mut output = PtpWavOutput {
                timing: &timing,
                transport: self
                    .realtime_transport
                    .as_ref()
                    .context("transport absent")?,
                events: self.events.as_ref(),
                lease: &lease,
                cancel: &self.cancel,
                feedback: &mut feedback,
            };
            let result = if let Some(frames) = frames {
                super::playback::run(&mut output, &mut encoder, frames, &mut self.playback_stats)
            } else {
                // PTP is serviced by the scoped worker throughout capture
                // startup. Never spend the prefill timeout with timing idle.
                let mut feedback_at = Instant::now() + Duration::from_secs(2);
                let ready =
                    capture
                        .as_ref()
                        .context("capture absent")?
                        .wait_ready(&self.cancel, || {
                            output.check()?;
                            if Instant::now() >= feedback_at {
                                output.feedback()?;
                                feedback_at = Instant::now() + Duration::from_secs(2);
                            }
                            Ok(())
                        });
                let start = Instant::now();
                let result = ready.and_then(|()| {
                    super::playback::run_optional_source(
                        &mut output,
                        &mut encoder,
                        &mut self.playback_stats,
                        start,
                        Instant::now,
                        std::thread::park_timeout,
                        limit.map(|n| n + u64::from(super::realtime::PLAYBACK_LEAD_SAMPLES)),
                        super::playback::Policy::Live,
                        |sent| {
                            if limit.is_some_and(|n| sent >= n) {
                                capture.take();
                                return Ok(Some([[0; 2]; profile::SPF]));
                            }
                            if capture.as_ref().context("capture absent")?.ended()? {
                                println!("Silence threshold reached or capture epoch reset; draining AirPlay timeline");
                                return Ok(None);
                            }
                            if sent == 0 {
                                println!("AirPlay streaming started");
                            }
                            let mut block = capture.as_mut().context("capture absent")?.block()?;
                            if let Some(n) = limit {
                                for (offset, frame) in block.iter_mut().enumerate() {
                                    if sent + offset as u64 >= n {
                                        *frame = [0; 2];
                                    }
                                }
                            }
                            Ok(Some(block))
                        },
                    )
                });
                self.playback_stats.elapsed = start.elapsed();
                encoder.flush();
                result
            };
            drop(stop);
            let worker = worker
                .join()
                .map_err(|_| anyhow::anyhow!("PTP playback worker panicked"))
                .and_then(|r| r);
            result.and(worker)
        });
        // Drop the session input before reclaiming control and bounded TEARDOWN.
        // Manual input stops/joins ALSA; automatic readers leave the outer
        // monitoring owner alive, including network and cancellation failures.
        drop(capture);
        self.timing = Some(match timing.into_inner() {
            Ok(timing) => timing,
            Err(poisoned) => {
                result = Err(anyhow::anyhow!("PTP timing poisoned"));
                poisoned.into_inner()
            }
        });
        self.realtime_encoder = Some(encoder);
        let (wire, token, outcome) = feedback.reclaim();
        self.control = wire;
        if token != self.token {
            result = Err(anyhow::anyhow!("feedback changed session binding"));
        }
        result.and(outcome)
    }
    fn call(
        &mut self,
        method: &str,
        path: Option<&str>,
        body: &[u8],
    ) -> Result<super::wire::Reply> {
        if method != "TEARDOWN" {
            self.attempted = match (method, path) {
                ("GET", _) => "info",
                ("RECORD", _) => "RECORD",
                ("SETPEERS", _) => "SETPEERS",
                ("POST", Some("/command")) => "command registration",
                ("POST", Some("/audioMode")) => "audioMode",
                ("SETUP", _) if self.stage.is_some() => "stream SETUP",
                ("SETUP", _) => "initial SETUP",
                ("SETRATEANCHORTIME", _) => "paused anchor",
                _ => "control",
            };
            self.check()?;
        }
        let mut headers = Vec::new();
        if method == "SETPEERS" {
            headers.push(("Content-Type", "/peer-list-changed".to_owned()));
        } else if !body.is_empty() {
            headers.push((
                "Content-Type",
                "application/x-apple-binary-plist".to_owned(),
            ));
        }
        if let Some(token) = &self.token {
            headers.push(("Session", token.clone()));
        }
        // A scoped worker services the owned timing sockets throughout blocking
        // control I/O. Join before interpreting the response or advancing state.
        let wire = self.control.as_mut().context("control absent")?;
        let uri = path.unwrap_or(&self.uri);
        let result = if let Some(timing) = self.timing.as_mut() {
            if method == "TEARDOWN" {
                timing.during_cleanup(&self.cancel, || wire.request(method, uri, &headers, body))
            } else {
                timing.during_control(&self.cancel, || wire.request(method, uri, &headers, body))
            }
        } else {
            wire.request(method, uri, &headers, body)
        };
        self.rtsp_stages.push(format!(
            "{}: {}",
            if method == "TEARDOWN" {
                "TEARDOWN"
            } else {
                self.attempted
            },
            if result.is_ok() {
                "RTSP accepted"
            } else {
                "request/timing failed"
            }
        ));
        let reply = result?;
        // Only initial SETUP may establish a token. Omission later keeps the
        // original binding; a tokenless session cannot acquire a token late.
        // Timing state separately proves semantic SETUP/resource validation.
        let initial_setup = method == "SETUP"
            && self.state == State::Negotiation
            && self.attempted_setup
            && !self.session_bound
            && self.stage.is_none();
        if initial_setup {
            self.setup_rtsp_accepted = true;
        }
        if let Some(token) = reply.headers.get("session") {
            // Strict diagnostic subset: parameterized sessions are unsupported.
            ensure!(
                !token.is_empty()
                    && token.len() <= 128
                    && token
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
                "unsupported receiver session token"
            );
            ensure!(
                initial_setup || self.token.as_ref() == Some(token),
                "receiver session changed or unexpectedly introduced"
            );
            if initial_setup {
                self.token = Some(token.clone());
            }
        }
        if initial_setup {
            self.session_bound = true;
        }
        Ok(reply)
    }
    fn plist(reply: &super::wire::Reply) -> Result<Value> {
        ensure!(
            reply
                .headers
                .get("content-type")
                .is_some_and(|v| v == "application/x-apple-binary-plist"),
            "expected binary plist content type"
        );
        profile::parse(&reply.body)
    }
    fn negotiate(&mut self) -> Result<()> {
        self.negotiate_with(|wire, setup, clock_id| {
            let events = wire.event(setup.event_port)?;
            let timing = wire.start_ptp(
                clock_id,
                setup.clock_id,
                setup.peer,
                Duration::from_secs(30),
            )?;
            Ok((Some(events), timing))
        })
    }
    fn negotiate_with(
        &mut self,
        resources: impl FnOnce(&mut Wire, InitialSetup, u64) -> Result<(Option<Worker>, ptp::Service)>,
    ) -> Result<()> {
        self.check()?;
        let info = Self::plist(&self.call("GET", Some("/info"), &[])?)?;
        let wire = self.control.as_ref().context("control absent")?;
        validate_info_profile(
            &info,
            wire.receiver_id(),
            wire.stored_hap_verification(),
            self.realtime_profile,
        )?;
        self.last_accepted = "info";
        let local = wire.local_ip()?;
        let peer = wire.ptp_lease()?.peer();
        let authority = match local {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        };
        self.uri = format!("rtsp://{authority}/{}", self.uuid);
        let offer = initial_offer(&wire.sender_device_id(), local, self.clock_id, self.uuid)?;
        // Set before sending: semantic errors after RTSP 200 still need TEARDOWN.
        self.attempted_setup = true;
        let reply = self.call("SETUP", None, &offer)?;
        let response = Self::plist(&reply)?;
        let setup = InitialSetup::parse(&response, peer, self.clock_id)?;
        self.peer_clock_id = Some(setup.clock_id);
        self.last_accepted = "initial SETUP";
        self.check()?;
        let wire = self.control.as_mut().context("control absent")?;
        let (events, timing) = resources(wire, setup, self.clock_id)?;
        self.events = events;
        self.timing = Some(ptp::session::TimingSession::attach(timing));
        self.state.advance(State::Timing)
    }
    /// All stages require a separately confirmed CLI opt-in. Never opens a media
    /// socket or exposes a playback grant, even after anchor acknowledgement.
    pub fn progress(&mut self, target: Stage) -> Result<()> {
        ensure!(
            !self.realtime_profile,
            "buffered stages unavailable for realtime profile"
        );
        let result = self.progress_inner(target);
        self.finish(result)
    }
    fn ack(&mut self, method: &str, path: Option<&str>, body: &[u8]) -> Result<()> {
        let reply = self.call(method, path, body)?;
        ptp::control::validate_reply(&reply, None)?;
        self.last_accepted = self.attempted;
        Ok(())
    }
    fn progress_inner(&mut self, target: Stage) -> Result<()> {
        ensure!(
            self.state == State::Timing && self.stage.is_none(),
            "control stages already used or unavailable"
        );
        ensure!(
            self.session_bound,
            "validated initial SETUP session binding required for progression"
        );
        self.ack("RECORD", None, &[])?;
        self.stage = Some(Stage::Record);
        if target == Stage::Record {
            return Ok(());
        }
        self.set_timing_peers()?;
        // This CLI implements no MediaRemote remote-control commands. Register
        // an honest empty table rather than advertising unsupported operations.
        let commands = command_registration()?;
        self.ack("POST", Some("/command"), &commands)?;
        let mut prepared = Prepared::new()?;
        let connection_id = rand::random::<i64>();
        let offer = prepared.setup_offer(connection_id)?;
        let response = Self::plist(&self.call("SETUP", None, &offer)?)?;
        let stream = NegotiatedStream::parse(&response, connection_id)?;
        self.last_accepted = "stream SETUP";
        self.prepared = Some(prepared);
        self.stream = Some(stream);
        let mode = profile::binary(&mut profile::dict([(
            "audioMode",
            Value::String("default".into()),
        )]))?;
        self.ack("POST", Some("/audioMode"), &mode)?;
        self.stage = Some(Stage::Stream);
        if target == Stage::Stream {
            return Ok(());
        }
        let now = Instant::now();
        let anchor = self
            .timing
            .as_mut()
            .context("timing absent")?
            .paused_anchor(0, now)?;
        let body = anchor.encode()?;
        let reply = self.call("SETRATEANCHORTIME", None, &body)?;
        ptp::control::validate_reply(&reply, Some(&profile::parse(&body)?))?;
        self.timing
            .as_mut()
            .context("timing absent")?
            .validate_anchor(&anchor, Instant::now())?;
        self.last_accepted = "paused anchor";
        self.anchor_accepted = true;
        self.stage = Some(Stage::Anchor);
        Ok(())
    }
    /// OwnTone orders initial SETUP, RECORD, then SETPEERS. Stop here for the
    /// timing-only diagnostic: no stream key, stream SETUP or media connection.
    #[cfg(test)]
    pub fn negotiate_timing_peers(&mut self) -> Result<()> {
        let result = (|| {
            self.progress_inner(Stage::Record)?;
            self.set_timing_peers()
        })();
        self.finish(result)
    }
    fn set_timing_peers(&mut self) -> Result<()> {
        let wire = self.control.as_ref().context("control absent")?;
        let peers = ptp::control::Peers::new(
            self.timing.as_ref().context("timing absent")?.peer(),
            wire.local_ip()?,
        )?
        .encode()?;
        self.ack("SETPEERS", None, &peers)
    }
    /// Observe timing on the owning thread between control requests.
    /// Peer traffic is reported only as activity, never synchronization.
    pub fn observe(&mut self, duration: Duration) -> Result<ptp::State> {
        ensure!(
            self.state == State::Timing && !self.observed,
            "timing diagnostic not active or already observed"
        );
        self.observed = true;
        let result = (|| {
            ensure!(
                !duration.is_zero() && duration <= Duration::from_secs(20),
                "timing observation must be at most twenty seconds"
            );
            let end = Instant::now() + duration;
            loop {
                self.check()?;
                self.timing
                    .as_mut()
                    .context("timing absent")?
                    .poll(&self.cancel)?;
                if Instant::now() >= end {
                    break;
                }
                std::thread::park_timeout(Duration::from_millis(2));
            }
            self.observed_peer_liveness = Some(
                self.timing
                    .as_ref()
                    .context("timing absent")?
                    .peer_is_live(Instant::now()),
            );
            if self.realtime_profile {
                self.realtime_ready()?;
                self.realtime_ready_snapshot = true;
            }
            self.observation_complete = true;
            Ok(self.timing.as_ref().context("timing absent")?.state())
        })();
        match result {
            Ok(state) => Ok(state),
            Err(error) => {
                self.finish(Err(error))?;
                unreachable!()
            }
        }
    }
    #[cfg(test)]
    fn request_playback(&mut self) -> Result<()> {
        self.finish(Err(anyhow::anyhow!(PLAYBACK_BLOCKER)))
    }
    pub fn stop(&mut self) -> Result<()> {
        ensure!(
            !matches!(self.state, State::Closed | State::Failed | State::Stopping),
            "buffered session already closed"
        );
        let result = self.cleanup();
        self.state.advance(if result.is_ok() {
            State::Closed
        } else {
            State::Failed
        })?;
        result
    }
    fn cleanup(&mut self) -> Result<()> {
        if !matches!(self.state, State::Closed | State::Failed | State::Stopping) {
            self.state.advance(State::Stopping)?;
        }
        if let Some(timing) = &self.timing {
            self.timing_health_snapshot = timing.healthy(Instant::now());
            self.final_ptp = Some(timing.state());
            self.final_evidence = timing.evidence();
            if self.observed_peer_liveness.is_none() {
                self.observed_peer_liveness = Some(timing.peer_is_live(Instant::now()));
            }
        }
        // Healthy timing continues during bounded teardown. A cancelled/lost
        // service must not prevent an otherwise authorized cleanup request.
        if self.cancel.load(Ordering::Relaxed)
            || self
                .timing
                .as_ref()
                .is_some_and(|s| matches!(s.state(), ptp::State::Lost | ptp::State::Shutdown))
        {
            if let Some(mut timing) = self.timing.take() {
                timing.shutdown();
                self.final_ptp = Some(timing.state());
            }
        }
        self.realtime_transport.take();
        self.realtime_encoder.take();
        self.prepared.take();
        self.stream.take();
        self.anchor_accepted = false;
        let result = (|| {
            if self.attempted_setup {
                self.teardown = "unacknowledged; remote expiry unconfirmed";
                self.attempted_setup = false;
                let wire = self.control.as_mut().context("control absent")?;
                wire.limit_cleanup();
                ensure!(
                    !wire.is_invalidated(),
                    "TEARDOWN unavailable: control invalidated; remote expiry unconfirmed"
                );
                let body = profile::binary(&mut profile::dict([]))?;
                let reply = self.call("TEARDOWN", None, &body)?;
                ptp::control::acknowledgement(&reply.body, None)?;
                self.teardown = "acknowledged";
            }
            Ok(())
        })();
        if let Some(mut timing) = self.timing.take() {
            self.final_evidence = timing.evidence();
            timing.shutdown();
            self.final_ptp = Some(timing.state());
        }
        self.events.take();
        if let Some(mut wire) = self.control.take() {
            wire.close();
        }
        self.token = None;
        self.session_bound = false;
        result
    }
    fn finish(&mut self, result: Result<()>) -> Result<()> {
        if let Err(error) = result {
            let cleanup = self.cleanup();
            self.state = State::Failed;
            return match cleanup {
                Ok(()) => Err(error),
                Err(cleanup) => Err(error.context(format!("cleanup: {cleanup:#}"))),
            };
        }
        Ok(())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

struct PtpWavOutput<'a> {
    timing: &'a std::sync::Mutex<ptp::session::TimingSession>,
    transport: &'a super::wire::RealtimeTransport,
    events: Option<&'a Worker>,
    lease: &'a super::wire::PtpLease,
    cancel: &'a AtomicBool,
    feedback: &'a mut super::session::FeedbackWorker,
}
impl super::playback::Output for PtpWavOutput<'_> {
    fn check(&mut self) -> Result<()> {
        ensure!(!self.cancel.load(Ordering::Relaxed), "playback cancelled");
        self.lease.check()?;
        if let Some(events) = self.events {
            events.check()?;
        }
        self.feedback.check()?;
        self.transport.check()?;
        let timing = self
            .timing
            .lock()
            .map_err(|_| anyhow::anyhow!("PTP timing poisoned"))?;
        let now = Instant::now();
        ensure!(
            timing.healthy(now) && timing.peer_is_live(now),
            "PTP timing lost during playback"
        );
        Ok(())
    }
    fn feedback(&mut self) -> Result<()> {
        self.feedback.request()
    }
    fn retransmit(&mut self) -> Result<Option<(u16, u16)>> {
        self.transport.retransmit()
    }
    fn send(&mut self, packet: &[u8]) -> Result<()> {
        self.transport.send(packet)
    }
    fn sync(&mut self, first: bool, current: u32, next: u32, at: Instant) -> Result<()> {
        let packet = self
            .timing
            .lock()
            .map_err(|_| anyhow::anyhow!("PTP timing poisoned"))?
            .realtime_sync(first, current, next, at)?;
        self.transport.sync(&packet)
    }
}

fn command_registration() -> Result<zeroize::Zeroizing<Vec<u8>>> {
    profile::binary(&mut profile::dict([
        ("type", Value::String("updateMRSupportedCommands".into())),
        (
            "params",
            profile::dict([("mrSupportedCommandsFromSender", Value::Array(vec![]))]),
        ),
    ]))
}

// Only fixed policy field names and numeric/boolean values may reach logs/errors.
// Never format arbitrary plist values: strings/data/containers can contain secrets.
fn info_policy_diagnostic(root: &plist::Dictionary) -> String {
    [
        "statusFlags",
        "acl",
        "act",
        "accessControlLevel",
        "accessControlType",
        "isGroupLeader",
        "isGroupMember",
        "isProxyGroupPlayer",
    ]
    .into_iter()
    .map(|field| {
        let value = match root.get(field) {
            None => "absent".into(),
            Some(Value::Integer(n)) => match n.as_unsigned() {
                Some(n) => format!("{n} (0x{n:x})"),
                None => format!("{} (negative)", n.as_signed().expect("plist integer")),
            },
            Some(Value::Boolean(b)) => b.to_string(),
            Some(_) => "invalid-type".into(),
        };
        format!("{field}={value}")
    })
    .collect::<Vec<_>>()
    .join("; ")
}

#[cfg(test)]
fn validate_info(
    value: &Value,
    receiver_id: &str,
    verified_stored_hap: Option<&pairing::VerifiedStoredHap>,
) -> Result<()> {
    validate_info_profile(value, receiver_id, verified_stored_hap, false)
}
fn validate_info_profile(
    value: &Value,
    receiver_id: &str,
    verified_stored_hap: Option<&pairing::VerifiedStoredHap>,
    realtime_profile: bool,
) -> Result<()> {
    let root = profile::dictionary(value)?;
    let id = root
        .get("deviceID")
        .and_then(Value::as_string)
        .and_then(crate::config::device_id)
        .context("authenticated /info deviceID absent or invalid")?;
    ensure!(id == receiver_id, "authenticated receiver identity changed");
    let evidence = Evidence::authenticated_info(value);
    let Claim::Observed(features) = evidence.features else {
        bail!("authenticated features unavailable");
    };
    if realtime_profile {
        ensure!(features & (1u64 << 41) != 0, "PTP required; no fallback");
        profile::ptp_capabilities(value)?;
    } else {
        ensure!(
            features & ((1u64 << 40) | (1u64 << 41)) == ((1u64 << 40) | (1u64 << 41)),
            "buffered audio and PTP required; no fallback"
        );
        let format_diagnostic = super::planning::buffered_format_diagnostic(value);
        tracing::debug!(target: "piair::rtsp", formats = %format_diagnostic,
        "authenticated buffered format reconciliation");
        ensure!(
            matches!(evidence.buffered_formats, Claim::Observed(ref formats) if formats.contains(&21)),
            "authenticated ALAC24/48000/stereo format unavailable or contradictory: {}",
            format_diagnostic
        );
    }
    // Separate authenticated observation, never substituted from mDNS flags/sf.
    // One-time pairing is satisfied only by pinned StoredHap verification on
    // this control wire. Method selection and transient encryption are insufficient.
    let policy = info_policy_diagnostic(root);
    tracing::debug!(target: "piair::rtsp", policy = %policy,
        "authenticated /info access policy");
    let flags = profile::integer(root, "statusFlags")
        .with_context(|| format!("invalid authenticated access policy: rejected field=statusFlags; expected unsigned integer; {policy}"))?;
    let pairing_required = if verified_stored_hap.is_some() {
        0
    } else {
        1 << 9
    };
    let rejected = flags & ((1 << 3) | (1 << 7) | pairing_required);
    ensure!(
        rejected == 0,
        "unsupported authenticated access policy: rejected field=statusFlags; rejected mask=0x{rejected:x} (bit 3=PIN, bit 7=password, bit 9=one-time pairing); {policy}"
    );
    for field in ["acl", "act", "accessControlLevel", "accessControlType"] {
        if root.contains_key(field) {
            ensure!(
                profile::integer(root, field)
                    .with_context(|| format!("invalid authenticated access policy: rejected field={field}; {policy}"))? == 0,
                "unsupported authenticated access policy: rejected field={field}; expected zero; {policy}"
            );
        }
    }
    for field in ["isGroupLeader", "isGroupMember", "isProxyGroupPlayer"] {
        if let Some(v) = root.get(field) {
            ensure!(
                v.as_boolean() == Some(false),
                "group/proxy receiver unsupported: rejected field={field}; expected false; {policy}"
            );
        }
    }
    Ok(())
}
fn initial_offer(
    device: &str,
    local: IpAddr,
    clock_id: u64,
    uuid: uuid::Uuid,
) -> Result<zeroize::Zeroizing<Vec<u8>>> {
    ensure!(
        clock_id != 0 && clock_id != u64::MAX,
        "invalid local ClockID"
    );
    ensure!(
        !local.is_unspecified() && !local.is_multicast(),
        "invalid local timing address"
    );
    let peer = profile::dict([
        ("ID", Value::String(uuid.to_string().to_uppercase())),
        ("DeviceType", profile::uint(0)),
        ("ClockID", Value::Integer((clock_id as i64).into())),
        ("SupportsClockPortMatchingOverride", Value::Boolean(false)),
        (
            "Addresses",
            Value::Array(vec![Value::String(local.to_string())]),
        ),
    ]);
    profile::binary(&mut profile::dict([
        ("name", Value::String("PiAir".into())),
        ("deviceID", Value::String(device.into())),
        (
            "sessionUUID",
            Value::String(uuid.to_string().to_uppercase()),
        ),
        ("timingProtocol", Value::String("PTP".into())),
        ("timingPeerInfo", peer.clone()),
        ("timingPeerList", Value::Array(vec![peer])),
    ]))
}
struct InitialSetup {
    peer: TimingPeer,
    event_port: u16,
    clock_id: u64,
}
impl InitialSetup {
    fn parse(value: &Value, peer: IpAddr, local_clock: u64) -> Result<Self> {
        let root = profile::dictionary(value)?;
        if let Some(protocol) = root.get("timingProtocol") {
            ensure!(
                protocol.as_string() == Some("PTP"),
                "receiver changed timing protocol"
            );
        }
        if root.contains_key("status") {
            ensure!(
                profile::integer(root, "status")? == 0,
                "initial SETUP rejected in-band"
            );
        }
        // PTP uses the standard ports; never interpret timingPort as a new peer.
        if root.contains_key("timingPort") {
            ensure!(
                profile::integer(root, "timingPort")? == 0,
                "unsupported PTP timing port"
            );
        }
        ensure!(
            !root.contains_key("streams"),
            "unexpected streams in initial SETUP"
        );
        let timing = profile::dictionary(
            root.get("timingPeerInfo")
                .context("timingPeerInfo absent")?,
        )?;
        if let Some(id) = timing.get("ID") {
            ensure!(
                id.as_string().is_some_and(|v| !v.is_empty()
                    && v.len() <= 128
                    && v.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b))),
                "invalid timing peer ID"
            );
        }
        if let Some(peers) = root.get("timingPeerList") {
            ensure!(
                peers
                    .as_array()
                    .is_some_and(|peers| peers.len() == 1 && peers[0] == root["timingPeerInfo"]),
                "unexpected additional timing peers"
            );
        }
        let clock = timing
            .get("ClockID")
            .and_then(Value::as_signed_integer)
            .map(|v| v as u64)
            .context("receiver ClockID unavailable; authenticated PTP identity binding unsupported; UDP trust-on-first-use disabled")?;
        ensure!(
            clock != 0 && clock != u64::MAX && clock != local_clock,
            "invalid receiver clock identity"
        );
        tracing::debug!(target: "piair::ptp", authorized_peer = %peer,
            expected_clock_id = format_args!("{clock:016x}"), "authenticated timing identity");
        let addresses = timing
            .get("Addresses")
            .and_then(Value::as_array)
            .context("timing addresses absent")?;
        let selected = TimingPeer::select(addresses, peer)
            .context("authenticated timing address validation failed")?;
        tracing::debug!(target: "piair::ptp", authorized_peer = %peer,
            selected_peer = %selected.0, expected_clock_id = format_args!("{clock:016x}"),
            reason = if selected.0 == peer { "authorized RTSP peer advertised" } else { "first compatible advertised peer" },
            "authenticated timing peer selected");
        Ok(Self {
            peer: selected,
            event_port: profile::port(root, "eventPort")?,
            clock_id: clock,
        })
    }
}

#[cfg(test)]
mod tests;

/// Address delegation is created only by authenticated SETUP parsing. The Wire
/// retains the original receiver authorization independently of this destination.
#[derive(Clone, Copy)]
pub(super) struct TimingPeer(IpAddr);
impl TimingPeer {
    pub(super) fn ip(self) -> IpAddr {
        self.0
    }
    pub(super) fn select(addresses: &[Value], authorized: IpAddr) -> Result<Self> {
        tracing::debug!(target: "piair::ptp", authorized_peer = %authorized,
            advertised_count = addresses.len(), "validating advertised timing addresses");
        ensure!(
            !addresses.is_empty() && addresses.len() <= 32,
            "timing address count outside 1..=32"
        );
        let mut candidates = Vec::new();
        for (index, value) in addresses.iter().enumerate() {
            // Never include unvalidated strings in diagnostics.
            let address = value
                .as_string()
                .filter(|s| s.len() <= 45)
                .and_then(|s| s.parse::<IpAddr>().ok());
            let Some(ip) = address else {
                tracing::debug!(target: "piair::ptp", index, reason = "malformed numeric IP", "timing address rejected");
                bail!("malformed timing address at index {index}");
            };
            let unsafe_address = ip.is_unspecified()
                || ip.is_multicast()
                || (ip.is_loopback() && !(cfg!(test) && ip == authorized))
                || match ip {
                    IpAddr::V4(v) => v.is_broadcast() || v.octets()[0] == 0 || v.octets()[0] >= 240,
                    IpAddr::V6(v) => v.to_ipv4_mapped().is_some(),
                };
            if unsafe_address {
                tracing::debug!(target: "piair::ptp", index, address = %ip, reason = "unsafe address", "timing address rejected");
                continue;
            }
            if ip.is_ipv4() != authorized.is_ipv4()
                || matches!(ip, IpAddr::V6(v) if v.is_unicast_link_local())
            {
                tracing::debug!(target: "piair::ptp", index, address = %ip, reason = "incompatible family or unsupported IPv6 scope", "timing address rejected");
                continue;
            }
            if !candidates.contains(&ip) {
                tracing::debug!(target: "piair::ptp", index, address = %ip, "validated timing candidate");
                candidates.push(ip);
            } else {
                tracing::debug!(target: "piair::ptp", index, address = %ip,
                    reason = "duplicate", "timing address ignored");
            }
        }
        let ip = candidates
            .iter()
            .find(|&&ip| ip == authorized)
            .or(candidates.first())
            .copied()
            .context("no usable advertised timing address")?;
        Ok(Self(ip))
    }
}
