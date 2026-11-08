//! Authenticated control owner. Audio negotiation is deliberately gated.
use super::{
    pairing, profile, realtime,
    store::Credential,
    wire::{MediaIo, Reply, Wire, Worker},
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

/// Setup-only comparisons; production playback retains the control lifecycle.
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

pub struct Session {
    state: State,
    setup_profile: SetupProfile,
    control: Option<Wire>,
    failure: Option<String>,
    uri: String,
    session_header: Option<String>,
    stream_id: Option<u32>,
    remote_attempted: bool,
    media: Option<MediaIo>,
    events: Option<Worker>,
    encoder: Option<realtime::Encoder>,
    clock: Option<Arc<realtime::Clock>>,
    latency: u32,
    pub packets_sent: u64,
    pub resends: u64,
    pub timing_exchanges: u64,
    pub cancel: Arc<AtomicBool>,
}
impl Session {
    pub fn new() -> Self {
        Self {
            state: State::Disconnected,
            setup_profile: SetupProfile::A,
            control: None,
            failure: None,
            uri: String::new(),
            session_header: None,
            stream_id: None,
            remote_attempted: false,
            media: None,
            events: None,
            encoder: None,
            clock: None,
            latency: profile::MIN_LATENCY,
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
    pub fn start_setup_selected(
        &mut self,
        connect: impl FnOnce() -> Result<Wire>,
        credential: Option<&Credential>,
        profile: SetupProfile,
    ) -> Result<()> {
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
        self.cancel.store(false, Ordering::Relaxed);
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
    pub fn negotiate_realtime(&mut self) -> Result<()> {
        ensure!(
            self.setup_profile == SetupProfile::A,
            "diagnostic profile is setup-only"
        );
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
            let encoder = realtime::Encoder::new()?;
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
            let mut ports = self.control.as_ref().unwrap().bind_media()?;
            let control_port = ports.control()?;
            let timing_port = ports.timing()?;
            if self.setup_profile != SetupProfile::A {
                stage = "early NTP worker startup";
                self.control.as_ref().unwrap().start_early_timing(
                    &mut ports, self.clock.as_ref().unwrap().clone(),
                )?;
                tracing::info!(target: "piair::rtsp", timing_port, ready = true,
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
            if self.setup_profile != SetupProfile::A {
                ports.check_timing()?;
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
            let receiver_timing = d
                .contains_key("timingPort")
                .then(|| profile::port(d, "timingPort"))
                .transpose()?;
            tracing::debug!(target: "piair::rtsp", phase = "initial SETUP", status = 200,
                timing_profile = "NTP", "initial SETUP profile and ports validated");
            if initial_only {
                return Ok(());
            }
            let receiver_timing = receiver_timing.context(
                "full playback requires receiver timingPort; probe-source NTP lifecycle is not implemented for playback",
            )?;
            status = None;
            stage = "NTP worker startup";
            self.control.as_ref().unwrap().start_timing(
                &mut ports,
                receiver_timing,
                self.clock.as_ref().unwrap().clone(),
            )?;
            status = None;
            stage = "event connection";
            self.events = Some(self.control.as_mut().unwrap().event(event_port)?);
            // The timing endpoint is authenticated before any timing reply is sent.
            // Stream ports arrive later; retain the bound sockets until then.
            status = None;
            stage = "RECORD";
            let record = self.call("RECORD", None, &[], &[])?;
            status = Some(200);
            self.latency = realtime::latency(&record.headers)?;
            status = None;
            stage = "stream SETUP";
            let offer = profile::stream_offer(encoder.key(), control_port, session_id)?;
            let stream = self.call("SETUP", None, &offer, &[])?;
            status = Some(200);
            let negotiated =
                profile::stream(&Self::plist_response(&stream).context("stream SETUP response")?)?;
            self.stream_id = negotiated.id;
            let (data, control) = (negotiated.data, negotiated.control);
            status = None;
            stage = "media socket connection";
            self.media = Some(self.control.as_ref().unwrap().media(
                ports,
                data,
                control,
                receiver_timing,
            )?);
            self.encoder = Some(encoder);
            status = None;
            stage = "SET_PARAMETER volume";
            let volume = self.call(
                "SET_PARAMETER",
                None,
                b"volume: -30.000000\r\n",
                &[("Content-Type", "text/parameters".into())],
            )?;
            status = Some(200);
            ensure!(volume.body.is_empty(), "unexpected volume response body");
            status = None;
            stage = "NTP/event synchronization";
            let deadline = Instant::now() + Duration::from_secs(3);
            while self.media.as_ref().unwrap().timing.count() == 0
                || self.events.as_ref().unwrap().count() == 0
            {
                self.workers()?;
                ensure!(
                    Instant::now() < deadline,
                    "synchronization wait timed out: ntp_ready={} event_ready={}",
                    self.media.as_ref().unwrap().timing.count() > 0,
                    self.events.as_ref().unwrap().count() > 0
                );
                std::thread::park_timeout(Duration::from_millis(5));
            }
            self.workers()?;
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
        self.finish(result)
    }
    fn workers(&self) -> Result<()> {
        ensure!(
            !self.cancel.load(Ordering::Relaxed),
            "diagnostic interrupted"
        );
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
    /// Signal amplitude <= -18 dBFS and receiver volume is explicitly set to -30dB.
    pub fn transmit_test(&mut self, seconds: u32) -> Result<()> {
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
            let signal = u64::from(seconds) * u64::from(profile::RATE);
            // Match OwnTone's NTP sync model: selected 2s lead minus receiver latency.
            let gap = profile::MAX_LATENCY - self.latency;
            let total = signal + u64::from(profile::MAX_LATENCY);
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
                if Instant::now() >= feedback_at {
                    self.feedback()?;
                    feedback_at += Duration::from_secs(2);
                }
                if let Some((first, count)) = self.media.as_ref().unwrap().retransmit()? {
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
                        gap,
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
            tracing::debug!(target: "piair::rtsp", "TEARDOWN skipped: control connection already invalidated; local resources will be released");
            return Ok(());
        }
        let body = profile::binary(&mut profile::dict([]))?;
        let reply = self.call("TEARDOWN", None, &body, &[])?;
        ensure!(reply.body.is_empty(), "unexpected TEARDOWN body");
        Ok(())
    }
    fn release(&mut self) {
        tracing::debug!(target: "piair::rtsp", "session cleanup: releasing sockets and joining workers");
        self.media = None;
        self.events = None;
        self.encoder = None;
        self.clock = None;
        self.control = None;
        tracing::debug!(target: "piair::rtsp", "session cleanup complete");
    }
    fn finish(&mut self, result: Result<()>) -> Result<()> {
        if let Err(error) = &result {
            if self.failure.is_none() {
                self.failure = Some(format!("{error:#}"));
            }
            // Cleanup must not spend a fresh request budget waiting for the peer.
            if let Some(control) = &mut self.control {
                control.limit_cleanup();
            }
            if self.teardown().is_err() {
                tracing::warn!("session cleanup failed; local resources will be released");
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
            if self.encoder.is_some() {
                self.flush()?;
            }
            self.state = State::Stopping;
            self.teardown()
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
