//! Authenticated control owner. Audio negotiation is deliberately gated.
use super::{
    pairing, profile, realtime,
    store::Credential,
    wire::{MediaIo, Ports, Reply, Wire, Worker},
};
use anyhow::{bail, ensure, Context, Result};
use plist::Value;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// Setup-only comparisons; playback always uses the early NTP responder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum SetupProfile {
    #[default]
    A,
    B,
    C,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Disconnected,
    Connecting,
    Authenticating,
    Negotiating,
    // Reserved until a verified buffered profile implements SETUP and RECORD.
    Ready,
    Streaming,
    Stopping,
    Failed,
}

pub(super) const RECORD_RECEIVER_ID: &str = "device:3ae5c9a302ad";

pub(super) const STUA_RECEIVER_ID: &str = "device:02d899c1833f";

pub struct Session {
    record_only: bool,
    stream_check: bool,
    playback_check: bool,
    record_ok: bool,
    state: State,
    volume_db: i32,
    setup_profile: SetupProfile,
    owntone_setup_only: bool,
    control: Option<Wire>,
    failure: Option<String>,
    uri: String,
    session_header: Option<String>,
    stream_id: Option<u32>,
    remote_attempted: bool,
    media: Option<MediaIo>,
    // Own sockets and the early worker through failed negotiation/TEARDOWN.
    // Successful media initialization moves them together into MediaIo.
    ports: Option<Ports>,
    events: Option<Worker>,
    encoder: Option<realtime::Encoder>,
    clock: Option<Arc<realtime::Clock>>,
    pub packets_sent: u64,
    pub resends: u64,
    pub timing_exchanges: u64,
    pub cancel: Arc<AtomicBool>,
}
impl Session {
    pub fn new() -> Self {
        Self {
            record_only: false,
            stream_check: false,
            playback_check: false,
            record_ok: false,
            state: State::Disconnected,
            volume_db: -30,
            setup_profile: SetupProfile::A,
            owntone_setup_only: false,
            control: None,
            failure: None,
            uri: String::new(),
            session_header: None,
            stream_id: None,
            remote_attempted: false,
            media: None,
            ports: None,
            events: None,
            encoder: None,
            clock: None,
            packets_sent: 0,
            resends: 0,
            timing_exchanges: 0,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
    /// Production caller must supply Wire::connect (the sole authorization boundary).
    /// No retry: a failed instance must be explicitly discarded.
    #[cfg(test)]
    pub fn start(
        &mut self,
        connect: impl FnOnce() -> Result<Wire>,
        credential: &Credential,
    ) -> Result<()> {
        self.start_selected(connect, Some(credential))
    }
    /// Own the full same-socket authentication lifecycle and retain its profile,
    /// verified proof, directional cipher counters and CSeq into setup-only negotiation.
    pub fn start_owntone_setup_selected(
        &mut self,
        connect: impl FnOnce() -> Result<Wire>,
        capabilities: &super::compatibility::Capabilities,
    ) -> Result<()> {
        ensure!(
            self.state == State::Disconnected,
            "session already started or failed"
        );
        capabilities.decide(super::compatibility::Method::Transient)?;
        self.owntone_setup_only = true;
        self.start_setup_selected(
            || {
                let mut wire = connect()?;
                wire.owntone_transient(capabilities)?;
                Ok(wire)
            },
            None,
            SetupProfile::A,
        )
    }
    /// Explicit RECORD diagnostic is limited to the approved HomePod identity.
    pub fn start_owntone_record_selected(
        &mut self,
        connect: impl FnOnce() -> Result<Wire>,
        capabilities: &super::compatibility::Capabilities,
    ) -> Result<()> {
        ensure!(
            self.state == State::Disconnected,
            "session already started or failed"
        );
        self.record_only = true;
        let cancel = self.cancel.clone();
        self.start_owntone_setup_selected(
            || {
                let mut wire = connect()?;
                wire.diagnostic_cancel = Some(cancel);
                ensure!(
                    wire.receiver_id() == RECORD_RECEIVER_ID,
                    "RECORD receiver ID is not approved for this diagnostic"
                );
                Ok(wire)
            },
            capabilities,
        )
    }
    pub fn start_owntone_stream_selected(
        &mut self,
        connect: impl FnOnce() -> Result<Wire>,
        capabilities: &super::compatibility::Capabilities,
        playback: bool,
    ) -> Result<()> {
        self.stream_check = true;
        self.playback_check = playback;
        self.start_owntone_record_selected(connect, capabilities)
    }
    /// Persistent HAP authentication feeds the same realtime diagnostic engine.
    pub fn start_stua_stream_selected(
        &mut self,
        connect: impl FnOnce() -> Result<Wire>,
        credential: &Credential,
        playback: bool,
    ) -> Result<()> {
        credential.validate(STUA_RECEIVER_ID)?;
        ensure!(
            self.state == State::Disconnected,
            "session already started or failed"
        );
        self.stream_check = true;
        self.playback_check = playback;
        self.record_only = true;
        let cancel = self.cancel.clone();
        self.start_selected(
            || {
                let mut wire = connect()?;
                ensure!(
                    wire.receiver_id() == STUA_RECEIVER_ID
                        && wire.method() != super::compatibility::Method::Transient,
                    "Stua diagnostic requires exact receiver and persistent HAP; no fallback"
                );
                wire.diagnostic_cancel = Some(cancel);
                Ok(wire)
            },
            Some(credential),
        )
    }
    pub fn check_stream(&mut self) -> Result<()> {
        ensure!(
            self.stream_check && !self.playback_check,
            "stream diagnostic required"
        );
        self.negotiate(false, None)?;
        self.stop()?;
        ensure!(
            !self.cancel.load(Ordering::Relaxed),
            "diagnostic interrupted"
        );
        Ok(())
    }
    pub fn prepare_playback_check(&mut self) -> Result<()> {
        ensure!(
            self.stream_check && self.playback_check,
            "playback diagnostic required"
        );
        self.volume_db = -40;
        self.negotiate(false, None)
    }
    pub fn record_succeeded(&self) -> bool {
        self.record_ok
    }
    pub fn check_record(&mut self) -> Result<()> {
        ensure!(
            self.record_only && self.owntone_setup_only,
            "explicit OwnTone RECORD diagnostic required"
        );
        self.negotiate(true, None)?;
        self.stop()?;
        let result = (|| {
            ensure!(
                !self.cancel.load(Ordering::Relaxed),
                "diagnostic interrupted"
            );
            Ok(())
        })();
        self.finish(result)
    }
    pub fn start_setup_selected(
        &mut self,
        connect: impl FnOnce() -> Result<Wire>,
        credential: Option<&Credential>,
        profile: SetupProfile,
    ) -> Result<()> {
        ensure!(
            !self.owntone_setup_only || profile == SetupProfile::A,
            "OwnTone transient setup requires profile A; no fallback"
        );
        self.setup_profile = profile;
        tracing::info!(target: "piair::rtsp", ?profile, "setup diagnostic selected");
        self.start_selected(connect, credential)
    }
    pub fn start_selected(
        &mut self,
        connect: impl FnOnce() -> Result<Wire>,
        credential: Option<&Credential>,
    ) -> Result<()> {
        ensure!(
            self.state == State::Disconnected,
            "session already started or failed"
        );
        if self.record_only {
            ensure!(
                !self.cancel.load(Ordering::Relaxed),
                "diagnostic interrupted"
            );
        } else {
            self.cancel.store(false, Ordering::Relaxed);
        }
        self.session_header = None;
        self.stream_id = None;
        self.uri.clear();
        self.packets_sent = 0;
        self.resends = 0;
        self.timing_exchanges = 0;
        self.state = State::Connecting;
        let result = (|| {
            let mut wire = connect()?;
            if self.setup_profile == SetupProfile::C {
                ensure!(
                    credential.is_some()
                        && wire.method() != super::compatibility::Method::Transient,
                    "profile C requires persistent credentials"
                );
                ensure!(
                    !self.cancel.load(Ordering::Relaxed),
                    "diagnostic interrupted"
                );
                // Untrusted capability metadata cannot authorize or install keys.
                let info = wire.preverify_info()?;
                Self::validate_info(&info)?;
            }
            tracing::debug!(target: "piair::rtsp", phase = "pair-verify", "authentication start");
            self.state = State::Authenticating;
            self.control = Some(
                if wire.method() == super::compatibility::Method::Transient {
                    ensure!(
                        credential.is_none(),
                        "transient session must not receive persistent credentials"
                    );
                    pairing::transient(wire)?
                } else {
                    pairing::verify(
                        wire,
                        credential.context("persistent credentials required; no fallback")?,
                    )?
                },
            );
            if self.record_only {
                self.control.as_mut().unwrap().diagnostic_cancel = Some(self.cancel.clone());
            }
            self.state = State::Negotiating;
            Ok(())
        })();
        self.finish(result)
    }
    pub fn check_control(&mut self) -> Result<()> {
        let result = (|| {
            ensure!(
                self.state == State::Negotiating,
                "session is not authenticated for negotiation"
            );
            self.control
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("control session absent"))?
                .options()
                .context("control check stage=OPTIONS method=OPTIONS")
        })();
        self.finish(result)
    }
    /// No SETUP, ports, key exchange, codec or timing profile has been validated.
    /// Calling this must never result in READY, RECORD, or plaintext media.
    #[allow(dead_code)] // Integration gate for PR #5; no CLI streaming command.
    pub fn start_buffered_audio(&mut self) -> Result<()> {
        let result = (|| {
            ensure!(
                self.state == State::Negotiating,
                "authenticated negotiation required"
            );
            bail!("buffered audio unsupported: type 103 codec offer, media key exchange, reserved header bits and Apple TV PTP/rate-anchor/feedback profile are unverified; standalone ALAC/packet components do not establish negotiation; FairPlay is not implemented")
        })();
        self.finish(result)
    }
    fn call(
        &mut self,
        method: &str,
        path: Option<&str>,
        body: &[u8],
        extra: &[(&str, String)],
    ) -> Result<Reply> {
        ensure!(
            !self.cancel.load(Ordering::Relaxed) || ["TEARDOWN", "FLUSH"].contains(&method),
            "diagnostic interrupted"
        );
        if !["TEARDOWN", "FLUSH"].contains(&method) {
            if let Some(ports) = &self.ports {
                if ports.has_timing_worker() {
                    ports.check_timing()?;
                }
            }
        }
        let uri = path.unwrap_or(&self.uri).to_owned();
        let mut headers = extra.to_vec();
        if !body.is_empty()
            && !extra
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        {
            headers.push(("Content-Type", "application/x-apple-binary-plist".into()));
        }
        if let Some(token) = &self.session_header {
            headers.push(("Session", token.clone()));
        }
        let response = self
            .control
            .as_mut()
            .context("control absent")?
            .request(method, &uri, &headers, body)
            .with_context(|| format!("RTSP method={method}"))?;
        (|| {
            if let Some(raw) = response.headers.get("session") {
                let mut parts = raw.split(';');
                let token = parts.next().unwrap_or("");
                ensure!(
                    !token.is_empty()
                        && token.len() <= 128
                        && token
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
                    "invalid receiver session identifier"
                );
                for parameter in parts {
                    let n = parameter
                        .trim()
                        .strip_prefix("timeout=")
                        .context("unsupported Session parameter")?;
                    ensure!(
                        n.parse::<u32>().is_ok_and(|n| (1..=3600).contains(&n)),
                        "invalid Session timeout"
                    );
                }
                ensure!(
                    self.session_header
                        .as_deref()
                        .is_none_or(|old| old == token),
                    "receiver session identifier changed"
                );
                self.session_header = Some(token.to_owned());
            }
            Ok(response)
        })()
        .with_context(|| format!("RTSP method={method} status=200 response validation failed"))
    }
    fn plist_response(reply: &Reply) -> Result<Value> {
        ensure!(
            reply
                .headers
                .get("content-type")
                .is_some_and(|s| s == "application/x-apple-binary-plist"),
            "expected binary-plist Content-Type"
        );
        profile::parse(&reply.body)
    }
    fn validate_info(info: &Reply) -> Result<()> {
        let info = Self::plist_response(info)?;
        profile::capabilities(&info)?;
        profile::select_ntp(&info)
    }
    /// Explicit concrete profile: HAP, ALAC/44100/16/2, realtime UDP, NTP.
    #[cfg(test)]
    pub fn negotiate_realtime(&mut self) -> Result<()> {
        self.negotiate_realtime_at_volume(-30)
    }
    pub fn negotiate_realtime_at_volume(&mut self, volume_db: i32) -> Result<()> {
        ensure!(
            (-60..=-10).contains(&volume_db),
            "volume must be -60..=-10 dB"
        );
        ensure!(
            self.setup_profile == SetupProfile::A
                && !self.stream_check
                && !self.owntone_setup_only
                && !self
                    .control
                    .as_ref()
                    .is_some_and(Wire::uses_owntone_transient_profile),
            "diagnostic profile is setup-only"
        );
        self.volume_db = volume_db;
        self.negotiate(false, None)
    }
    /// Validate the initial NTP SETUP only, then release the receiver session.
    pub fn check_initial_setup_with_timeout(&mut self, timeout: Option<Duration>) -> Result<()> {
        self.negotiate(true, timeout)?;
        self.stop()
    }
    fn negotiate(&mut self, initial_only: bool, setup_timeout: Option<Duration>) -> Result<()> {
        let mut stage = "local initialization";
        let mut status = None;
        let result = (|| {
            ensure!(
                self.state == State::Negotiating,
                "authenticated negotiation required"
            );
            ensure!(initial_only || self.stream_check || !self.owntone_setup_only, "OwnTone transient is setup-only");
            ensure!(initial_only || self.stream_check || !self.control.as_ref().is_some_and(Wire::uses_owntone_transient_profile),
                "OwnTone transient is setup-only");
            // Initial/RECORD-only checks never initialize the encoder. Stream checks
            // offer the verified key but generate no audio until readiness.
            let encoder = if initial_only { None } else if self.stream_check {
                Some(realtime::Encoder::from_verified_key(self.control.as_mut().context("control absent")?.take_audio_key()?)?)
            } else { Some(realtime::Encoder::new()?) };
            self.clock = Some(Arc::new(realtime::Clock::new()?));
            let session_id = rand::random::<u32>();
            let local = self
                .control
                .as_ref()
                .context("control absent")?
                .local_ip()?;
            self.uri = format!("rtsp://{local}/{session_id}");
            tracing::debug!(target: "piair::rtsp", uri = %self.uri, "session RTSP URI selected");
            status = None;
            stage = "GET /info";
            if self.setup_profile != SetupProfile::C {
                let info = self.call("GET", Some("/info"), &[], &[])?;
                status = Some(200);
                Self::validate_info(&info)?;
            }
            status = None;
            stage = "media socket binding";
            self.ports = Some(self.control.as_ref().unwrap().bind_media()?);
            let ports = self.ports.as_mut().unwrap();
            let control_port = ports.control()?;
            let timing_port = ports.timing()?;
            let early_timing = self.owntone_setup_only
                || !initial_only
                || self.setup_profile != SetupProfile::A;
            if early_timing {
                stage = "early NTP worker startup";
                self.control.as_ref().unwrap().start_timing(
                    ports, self.clock.as_ref().unwrap().clone(),
                )?;
                tracing::info!(target: "piair::rtsp", phase = "NTP", outcome = "started", timing_port, ready = true,
                    "NTP responder ready before initial SETUP");
            } else {
                tracing::info!(target: "piair::rtsp", timing_port, ready = false,
                    "control lifecycle: timing socket bound; responder deferred");
            }
            let uuid = uuid::Uuid::new_v4();
            let device = self.control.as_ref().unwrap().sender_device_id();
            let body = profile::ntp_offer(&device, uuid, timing_port)?;
            status = None;
            stage = "initial SETUP";
            tracing::debug!(target: "piair::rtsp", phase = "initial SETUP", timing_profile = "NTP",
                body_length = body.len(), timing_port,
                plist_fields = "deviceID:string,sessionUUID:string,timingProtocol:string,timingPort:unsigned-integer",
                sender_identity = "persistent client identity", "initial SETUP start");
            if early_timing {
                self.ports.as_ref().unwrap().check_timing()?;
            }
            self.remote_attempted = true;
            if let Some(timeout) = setup_timeout {
                ensure!(initial_only && timeout == Duration::from_secs(15), "invalid SETUP timeout experiment");
                self.control.as_mut().unwrap().setup_response_wait = Some((timeout, self.cancel.clone()));
            }
            let base = self.call("SETUP", None, &body, &[]);
            self.control.as_mut().unwrap().setup_response_wait = None;
            let base = base?;
            tracing::debug!(target: "piair::rtsp", phase = "initial SETUP", status = 200,
                "initial SETUP RTSP response validated");
            status = Some(200);
            let base = Self::plist_response(&base)?;
            let d = profile::dictionary(&base)?;
            if let Some(protocol) = d.get("timingProtocol") {
                ensure!(
                    protocol.as_string() == Some("NTP"),
                    "receiver requires unsupported PTP/timing profile; PiAir implements NTP only; use a sender with full PTP support"
                );
            }
            let event_port = profile::port(d, "eventPort")?;
            // OwnTone 29.3's initial NTP response requires only eventPort.
            // Validate an optional timingPort without inventing a remote endpoint.
            if d.contains_key("timingPort") {
                profile::port(d, "timingPort")?;
            }
            tracing::debug!(target: "piair::rtsp", phase = "initial SETUP", status = 200,
                timing_profile = "NTP", outcome = "validated", "initial SETUP profile and ports validated");
            if initial_only && !self.record_only {
                if self.owntone_setup_only {
                    ensure!(!self.cancel.load(Ordering::Relaxed), "diagnostic interrupted");
                }
                return Ok(());
            }
            status = None;
            stage = "event connection";
            self.workers()?;
            self.events = Some(self.control.as_mut().unwrap().event(event_port)?);
            tracing::debug!(target: "piair::rtsp", phase = "event connection", outcome = "connected", "event channel connected");
            // Timing probes are restricted to the verified control peer IP.
            // Stream ports arrive later; retain the bound sockets until then.
            status = None;
            stage = "RECORD";
            self.workers()?;
            tracing::debug!(target: "piair::rtsp", phase = "RECORD", outcome = "attempted", "RECORD request attempted");
            let record = self.call("RECORD", None, &[], &[])?;
            status = Some(200);
            if self.record_only {
                ensure!(record.body.is_empty(), "unexpected RECORD response body");
                if let Some(latency) = record.headers.get("audio-latency") {
                    ensure!(!latency.is_empty() && latency.bytes().all(|b| b.is_ascii_digit())
                        && latency.parse::<u32>().is_ok(), "invalid RECORD Audio-Latency");
                }
                self.workers()?;
                self.record_ok = true;
                tracing::info!(target: "piair::rtsp", phase = "RECORD", outcome = "validated", status = 200,
                    "RECORD validated; stream negotiation and playback unverified");
                if !self.stream_check { return Ok(()); }
            }
            realtime::record_latency(&record.headers);
            let encoder = encoder.context("playback encoder absent")?;
            status = None;
            stage = "stream SETUP";
            let offer = profile::stream_offer(encoder.key(), control_port, session_id)?;
            let stream = self.call("SETUP", None, &offer, &[])?;
            status = Some(200);
            let negotiated =
                profile::stream_for_connection(&Self::plist_response(&stream).context("stream SETUP response")?, session_id)?;
            ensure!(negotiated.data != event_port && negotiated.control != event_port,
                "media ports overlap event endpoint");
            self.workers()?;
            tracing::info!(target: "piair::rtsp", phase = "stream SETUP", outcome = "validated",
                data_port = negotiated.data, control_port = negotiated.control,
                sample_rate = profile::RATE, frames_per_packet = profile::SPF, payload_type = 96,
                "stream negotiation validated; audible playback unverified");
            self.stream_id = negotiated.id;
            if self.stream_check && !self.playback_check { return Ok(()); }
            let (data, control) = (negotiated.data, negotiated.control);
            status = None;
            stage = "media socket connection";
            self.media = Some(self.control.as_ref().unwrap().media(
                &mut self.ports,
                data,
                control,
            )?);
            self.encoder = Some(encoder);
            status = None;
            stage = "SET_PARAMETER volume";
            let volume = self.call(
                "SET_PARAMETER",
                None,
                format!("volume: {}.000000\r\n", self.volume_db).as_bytes(),
                &[("Content-Type", "text/parameters".into())],
            )?;
            status = Some(200);
            ensure!(volume.body.is_empty(), "unexpected volume response body");
            tracing::info!(target: "piair::rtsp", phase = "volume", volume_db = self.volume_db,
                may_persist = true, outcome = "validated", "receiver volume command acknowledged");
            status = None;
            stage = "NTP synchronization";
            let deadline = Instant::now() + Duration::from_secs(3);
            // An established event channel may remain idle indefinitely.
            // OwnTone continues without waiting for unsolicited traffic.
            while self.media.as_ref().unwrap().timing.count() == 0 {
                self.workers()?;
                ensure!(
                    Instant::now() < deadline,
                    "synchronization wait timed out: ntp_ready=false"
                );
                std::thread::park_timeout(Duration::from_millis(5));
            }
            self.workers()?;
            tracing::info!(target: "piair::rtsp", phase = "NTP synchronization", outcome = "ready",
                ntp_replies = self.media.as_ref().unwrap().timing.count(), "timing activity validated before audio");
            self.state = State::Ready;
            Ok(())
        })()
        .with_context(|| {
            // All labels are local constants; never format a URI, header or receiver value.
            let method = match stage {
                "GET /info" => " method=GET",
                "initial SETUP" | "stream SETUP" => " method=SETUP",
                "RECORD" => " method=RECORD",
                "SET_PARAMETER volume" => " method=SET_PARAMETER",
                _ => "",
            };
            match status {
                Some(code) => format!(
                    "realtime negotiation stage={stage}{method} status={code} response validation failed"
                ),
                None => format!("realtime negotiation stage={stage}{method}"),
            }
        });
        if self.record_only {
            if let Err(error) = &result {
                let outcome = error
                    .downcast_ref::<super::wire::ResponseFailure>()
                    .map_or("failed", super::wire::ResponseFailure::outcome);
                tracing::info!(target: "piair::rtsp", phase = stage, outcome, record_validated = self.record_ok, "OwnTone diagnostic failed");
            }
        }
        self.finish(result)
    }
    fn workers(&self) -> Result<()> {
        ensure!(
            !self.cancel.load(Ordering::Relaxed),
            "diagnostic interrupted"
        );
        if let Some(ports) = &self.ports {
            ports.check_timing().context("NTP worker")?;
        }
        if let Some(event) = &self.events {
            event.check().context("event worker")?;
        }
        if let Some(media) = &self.media {
            media.timing.check().context("NTP worker")?;
            ensure!(
                media.timing.age() < Duration::from_secs(5),
                "receiver NTP timing stalled"
            );
        }
        Ok(())
    }
    fn feedback(&mut self) -> Result<()> {
        let reply = self.call("POST", Some("/feedback"), &[], &[])?;
        if !reply.body.is_empty() {
            let value = Self::plist_response(&reply)?;
            let d = profile::dictionary(&value)?;
            let streams = d
                .get("streams")
                .and_then(Value::as_array)
                .context("invalid receiver feedback")?;
            ensure!(streams.len() <= 1, "unexpected feedback streams");
            for stream in streams {
                let d = profile::dictionary(stream)?;
                if d.contains_key("streamID") {
                    ensure!(
                        self.stream_id.map(u64::from) == Some(profile::integer(d, "streamID")?),
                        "feedback stream identifier changed"
                    );
                }
                if d.contains_key("type") {
                    ensure!(
                        profile::integer(d, "type")? == 96,
                        "feedback stream type changed"
                    );
                }
            }
        }
        Ok(())
    }
    /// Test input only. The transport, codec, crypto, sockets and timing are real.
    /// Signal amplitude <= -18 dBFS and receiver volume uses the explicitly authorized test setting.
    pub fn transmit_test(&mut self, seconds: u32) -> Result<()> {
        let attempted = Instant::now();
        let result = (|| {
            ensure!(
                self.state == State::Ready,
                "successful session negotiation required"
            );
            ensure!(
                (1..=3).contains(&seconds),
                "test duration must be 1..3 seconds"
            );
            let clock = self.clock.as_ref().context("session clock absent")?.clone();
            let start = Instant::now();
            let mut sent = 0u64;
            ensure!(
                !self.playback_check || seconds == 1,
                "playback check is fixed at one second"
            );
            let signal = u64::from(seconds) * u64::from(profile::RATE);
            // No unsent PCM at sync time: OwnTone's pos + input - output
            // reduces to next RTP timestamp minus our sender buffer.
            let total = signal + u64::from(realtime::PLAYBACK_LEAD_SAMPLES);
            let mut sync_at = 0;
            let mut feedback_at = Instant::now() + Duration::from_secs(2);
            let mut retry_budget = 1024;
            while sent < total {
                self.workers()?;
                let deadline = start + realtime::sample_time(sent);
                ensure!(
                    Instant::now().saturating_duration_since(deadline) < Duration::from_millis(100),
                    "audio scheduler stalled; no uncontrolled catch-up"
                );
                if !self.playback_check && Instant::now() >= feedback_at {
                    self.feedback()?;
                    feedback_at += Duration::from_secs(2);
                }
                if let Some((first, count)) = self.media.as_ref().unwrap().retransmit()? {
                    ensure!(
                        !self.playback_check,
                        "playback check forbids retransmissions"
                    );
                    ensure!(
                        retry_budget >= usize::from(count),
                        "session retransmission budget exhausted"
                    );
                    retry_budget -= usize::from(count);
                    for offset in 0..count {
                        if let Some(packet) = self
                            .encoder
                            .as_mut()
                            .unwrap()
                            .resend(first.wrapping_add(offset))?
                        {
                            self.media.as_ref().unwrap().send(packet)?;
                            self.resends += 1;
                        }
                    }
                }
                if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
                    std::thread::park_timeout(remaining.min(Duration::from_millis(5)));
                    continue;
                }
                ensure!(
                    Instant::now().saturating_duration_since(deadline) < Duration::from_millis(100),
                    "control feedback stalled the sample timeline"
                );
                if sent >= sync_at {
                    let next = self.encoder.as_ref().unwrap().timestamp;
                    self.media.as_ref().unwrap().sync(&realtime::sync(
                        sent == 0,
                        next,
                        clock.instant(start + realtime::sample_time(sent)),
                    ))?;
                    sync_at = sent + u64::from(profile::RATE);
                }
                let pcm: Vec<_> = (sent..sent + profile::SPF as u64)
                    .map(|n| {
                        if n >= signal {
                            [0, 0]
                        } else {
                            let phase = (n % 100) as i16;
                            let x = if phase < 50 {
                                phase * 164 - 4100
                            } else {
                                (100 - phase) * 164 - 4100
                            };
                            let x = if self.playback_check {
                                let fade =
                                    n.min(signal.saturating_sub(n)).min(2205) as f64 / 2205.0;
                                (f64::from(x) * fade) as i16
                            } else {
                                x
                            };
                            [x, x]
                        }
                    })
                    .collect();
                let packet = self.encoder.as_mut().unwrap().packet(&pcm)?;
                self.media.as_ref().unwrap().send(&packet)?;
                self.packets_sent += 1;
                self.state = State::Streaming;
                sent += profile::SPF as u64;
            }
            let drained = start + realtime::sample_time(total);
            while let Some(remaining) = drained.checked_duration_since(Instant::now()) {
                self.workers()?;
                std::thread::park_timeout(remaining.min(Duration::from_millis(5)));
            }
            self.timing_exchanges = self.media.as_ref().unwrap().timing.count();
            self.workers()?;
            Ok(())
        })();
        tracing::info!(target: "piair::rtsp", phase = "playback", rtp_packets = self.packets_sent,
            ntp_replies = self.media.as_ref().map_or(0, |m| m.timing.count()),
            elapsed_ms = attempted.elapsed().as_millis() as u64, success = result.is_ok(),
            "bounded transmission outcome; audible playback unverified");
        self.finish(result)
    }
    pub fn flush(&mut self) -> Result<()> {
        let result = (|| {
            ensure!(
                [State::Ready, State::Streaming].contains(&self.state),
                "flush requires negotiated session"
            );
            let e = self.encoder.as_ref().context("media encoder absent")?;
            let info = format!("seq={};rtptime={}", e.sequence, e.timestamp);
            let reply = self.call("FLUSH", None, &[], &[("RTP-Info", info)])?;
            ensure!(reply.body.is_empty(), "unexpected FLUSH body");
            self.encoder.as_mut().unwrap().flush();
            self.state = State::Ready;
            Ok(())
        })();
        self.finish(result)
    }
    fn teardown(&mut self) -> Result<()> {
        if !self.remote_attempted {
            return Ok(());
        }
        self.remote_attempted = false;
        if self
            .control
            .as_ref()
            .is_none_or(|control| control.is_invalidated())
        {
            tracing::debug!(target: "piair::rtsp", outcome = "skipped", "TEARDOWN skipped: control connection already invalidated; local resources will be released");
            ensure!(
                !self.stream_check,
                "TEARDOWN unacknowledged: control invalidated"
            );
            return Ok(());
        }
        let body = profile::binary(&mut profile::dict([]))?;
        tracing::debug!(target: "piair::rtsp", phase = "TEARDOWN", outcome = "attempted",
            "TEARDOWN request attempted");
        let result = (|| {
            let reply = self.call("TEARDOWN", None, &body, &[])?;
            ensure!(reply.body.is_empty(), "unexpected TEARDOWN body");
            Ok(())
        })();
        let outcome = match &result {
            Ok(()) => "acknowledged",
            Err(error) => error
                .downcast_ref::<super::wire::ResponseFailure>()
                .map_or("failed", super::wire::ResponseFailure::outcome),
        };
        tracing::debug!(target: "piair::rtsp", phase = "TEARDOWN", outcome, success = result.is_ok(),
            "TEARDOWN outcome; local resources will be released");
        result
    }
    fn release(&mut self) {
        tracing::debug!(target: "piair::rtsp", phase = "cleanup", outcome = "started", "session cleanup: releasing sockets and joining workers");
        if self.owntone_setup_only {
            if let Some(ports) = &self.ports {
                tracing::debug!(target: "piair::rtsp", phase = "NTP", started = ports.has_timing_worker(),
                    replies = ports.timing_replies(), "setup-only NTP responder cleanup");
            }
        }
        tracing::info!(target: "piair::rtsp", phase = "cleanup", rtp_packets = self.packets_sent,
            ntp_replies = self.media.as_ref().map_or_else(|| self.ports.as_ref().map_or(0, Ports::timing_replies), |m| m.timing.count()),
            "local resource release");
        self.media = None;
        self.ports = None;
        self.events = None;
        self.encoder = None;
        self.clock = None;
        self.control = None;
        tracing::debug!(target: "piair::rtsp", phase = "cleanup", outcome = "complete", "session cleanup complete");
    }
    fn finish(&mut self, mut result: Result<()>) -> Result<()> {
        if let Err(error) = &result {
            if self.failure.is_none() {
                self.failure = Some(format!("{error:#}"));
            }
            // Cleanup must not spend a fresh request budget waiting for the peer.
            if let Some(control) = &mut self.control {
                control.limit_cleanup();
            }
            if let Err(cleanup) = self.teardown() {
                tracing::warn!("session cleanup failed; local resources will be released");
                if self.record_only {
                    result = result.context(format!("bounded TEARDOWN also failed: {cleanup:#}"));
                }
            }
            self.release();
            self.state = State::Failed;
        }
        result
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
    pub fn stop(&mut self) -> Result<()> {
        if self.state == State::Disconnected || self.state == State::Failed {
            self.release();
            return Ok(());
        }
        let result = (|| {
            if self.encoder.is_some() && !self.playback_check {
                self.flush()?;
            }
            self.state = State::Stopping;
            self.teardown()?;
            ensure!(
                !self.stream_check || !self.cancel.load(Ordering::Relaxed),
                "diagnostic interrupted"
            );
            Ok(())
        })(); // flush failure already cleans up
        if result.is_err() {
            return self.finish(result);
        }
        self.release();
        self.state = State::Disconnected;
        Ok(())
    }
    pub fn shutdown(&mut self) {
        let _ = self.stop();
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.shutdown();
    }
}
