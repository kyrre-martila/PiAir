//! Persistent authenticated control owner. Audio negotiation is deliberately gated.
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
    pub fn start(
        &mut self,
        connect: impl FnOnce() -> Result<Wire>,
        credential: &Credential,
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
            let wire = connect()?;
            self.state = State::Authenticating;
            self.control = Some(pairing::verify(wire, credential)?);
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
            .request(method, &uri, &headers, body)?;
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
    /// Explicit concrete profile: HAP, ALAC/44100/16/2, realtime UDP, NTP.
    pub fn negotiate_realtime(&mut self) -> Result<()> {
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
            let info = self.call("GET", Some("/info"), &[], &[])?;
            profile::capabilities(&Self::plist_response(&info)?)?;
            let mut ports = self.control.as_ref().unwrap().bind_media()?;
            let control_port = ports.control()?;
            let timing_port = ports.timing()?;
            let uuid = uuid::Uuid::new_v4();
            let b = uuid.as_bytes();
            let device = format!(
                "02:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                b[0], b[1], b[2], b[3], b[4]
            );
            let body = profile::binary(&mut profile::dict([
                ("deviceID", Value::String(device)),
                (
                    "sessionUUID",
                    Value::String(uuid.to_string().to_uppercase()),
                ),
                ("timingProtocol", Value::String("NTP".into())),
                ("timingPort", profile::uint(u64::from(timing_port))),
            ]))?;
            self.remote_attempted = true;
            let base = self.call("SETUP", None, &body, &[])?;
            let base = Self::plist_response(&base)?;
            let d = profile::dictionary(&base)?;
            if let Some(protocol) = d.get("timingProtocol") {
                ensure!(
                    protocol.as_string() == Some("NTP"),
                    "receiver requires unsupported PTP/timing profile"
                );
            }
            let event_port = profile::port(d, "eventPort")?;
            let receiver_timing = profile::port(d, "timingPort")?;
            self.control.as_ref().unwrap().start_timing(
                &mut ports,
                receiver_timing,
                self.clock.as_ref().unwrap().clone(),
            )?;
            self.events = Some(self.control.as_mut().unwrap().event(event_port)?);
            // The timing endpoint is authenticated before any timing reply is sent.
            // Stream ports arrive later; retain the bound sockets until then.
            let record = self.call("RECORD", None, &[], &[])?;
            self.latency = realtime::latency(&record.headers)?;
            let offer = profile::stream_offer(encoder.key(), control_port, session_id)?;
            let stream = self.call("SETUP", None, &offer, &[])?;
            let negotiated =
                profile::stream(&Self::plist_response(&stream).context("stream SETUP response")?)?;
            self.stream_id = negotiated.id;
            let (data, control) = (negotiated.data, negotiated.control);
            self.media = Some(self.control.as_ref().unwrap().media(
                ports,
                data,
                control,
                receiver_timing,
            )?);
            self.encoder = Some(encoder);
            let volume = self.call(
                "SET_PARAMETER",
                None,
                b"volume: -30.000000\r\n",
                &[("Content-Type", "text/parameters".into())],
            )?;
            ensure!(volume.body.is_empty(), "unexpected volume response body");
            let deadline = Instant::now() + Duration::from_secs(3);
            while self.media.as_ref().unwrap().timing.count() == 0
                || self.events.as_ref().unwrap().count() == 0
            {
                self.workers()?;
                ensure!(
                    Instant::now() < deadline,
                    "receiver did not complete NTP synchronization and initial event"
                );
                std::thread::park_timeout(Duration::from_millis(5));
            }
            self.workers()?;
            self.state = State::Ready;
            Ok(())
        })();
        self.finish(result)
    }
    fn workers(&self) -> Result<()> {
        ensure!(
            !self.cancel.load(Ordering::Relaxed),
            "diagnostic interrupted"
        );
        if let Some(event) = &self.events {
            event.check()?;
        }
        if let Some(media) = &self.media {
            media.timing.check()?;
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
        let body = profile::binary(&mut profile::dict([]))?;
        let reply = self.call("TEARDOWN", None, &body, &[])?;
        ensure!(reply.body.is_empty(), "unexpected TEARDOWN body");
        Ok(())
    }
    fn release(&mut self) {
        self.media = None;
        self.events = None;
        self.encoder = None;
        self.clock = None;
        self.control = None;
    }
    fn finish(&mut self, result: Result<()>) -> Result<()> {
        if let Err(error) = &result {
            if self.failure.is_none() {
                self.failure = Some(format!("{error:#}"));
            }
            let _ = self.teardown();
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
