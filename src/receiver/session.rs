//! Authentication and exclusive control-channel handoff to realtime PTP.
use super::{
    pairing, profile,
    store::Credential,
    wire::{Reply, Wire},
};
use anyhow::{bail, ensure, Context, Result};
use plist::Value;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Disconnected,
    Connecting,
    Authenticating,
    Negotiating,
    Failed,
}

pub struct Session {
    state: State,
    control: Option<Wire>,
    failure: Option<String>,
    pub cancel: Arc<AtomicBool>,
}
impl Session {
    pub fn new() -> Self {
        Self {
            state: State::Disconnected,
            control: None,
            failure: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
    #[cfg(test)]
    pub fn start(
        &mut self,
        connect: impl FnOnce() -> Result<Wire>,
        credential: &Credential,
    ) -> Result<()> {
        self.start_selected(connect, Some(credential))
    }
    #[cfg(test)]
    pub fn start_selected(
        &mut self,
        connect: impl FnOnce() -> Result<Wire>,
        credential: Option<&Credential>,
    ) -> Result<()> {
        self.start_selected_with_cancel(connect, credential, false)
    }
    fn start_selected_with_cancel(
        &mut self,
        connect: impl FnOnce() -> Result<Wire>,
        credential: Option<&Credential>,
        preserve_cancel: bool,
    ) -> Result<()> {
        ensure!(
            self.state == State::Disconnected,
            "session already started or failed"
        );
        if preserve_cancel {
            ensure!(
                !self.cancel.load(Ordering::Relaxed),
                "authentication interrupted"
            );
        } else {
            self.cancel.store(false, Ordering::Relaxed);
        }
        self.state = State::Connecting;
        let result = (|| {
            let wire = connect()?;
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
    /// Authenticate once before the consuming PTP handoff, without NTP resources.
    pub fn authenticate_ptp_selected(
        &mut self,
        connect: impl FnOnce() -> Result<Wire>,
        credential: Option<&Credential>,
    ) -> Result<()> {
        let cancel = self.cancel.clone();
        ensure!(
            !cancel.load(Ordering::Relaxed),
            "PTP authentication cancelled"
        );
        self.start_selected_with_cancel(
            || {
                let mut wire = connect()?;
                wire.ptp_authorization()?;
                wire.diagnostic_cancel = Some(cancel);
                ensure!(
                    matches!(
                        wire.method(),
                        super::compatibility::Method::StoredHap
                            | super::compatibility::Method::Transient
                    ),
                    "PTP playback requires stored HAP or explicit transient authentication"
                );
                if wire.method() == super::compatibility::Method::Transient {
                    wire.ptp_transient()?;
                }
                Ok(wire)
            },
            credential,
            true,
        )
    }
    #[cfg(test)]
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

    pub(super) fn into_ptp_foundation(mut self) -> Result<super::lifecycle::Session> {
        self.check_ptp_handoff()?;
        let wire = self
            .control
            .take()
            .context("authenticated control absent")?;
        self.state = State::Disconnected;
        super::lifecycle::Session::from_authenticated(wire, self.cancel.clone())
    }
    fn check_ptp_handoff(&self) -> Result<()> {
        ensure!(
            self.state == State::Negotiating,
            "PTP handoff requires pristine authenticated session; no fallback"
        );
        ensure!(
            !self.cancel.load(Ordering::Relaxed),
            "PTP handoff cancelled"
        );
        Ok(())
    }
    fn validate_session_header(
        session_header: &mut Option<String>,
        response: Reply,
    ) -> Result<Reply> {
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
                session_header.as_deref().is_none_or(|old| old == token),
                "receiver session identifier changed"
            );
            *session_header = Some(token.to_owned());
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
    fn feedback_result(
        result: Result<Reply>,
        stream_id: Option<u32>,
        started: Instant,
    ) -> Result<()> {
        let status = match &result {
            Ok(_) => Some(200),
            Err(error) => match error.downcast_ref::<super::wire::ResponseFailure>() {
                Some(super::wire::ResponseFailure::Rejected(status)) => Some(*status),
                _ => None,
            },
        };
        let result = result.and_then(|reply| Self::validate_feedback(stream_id, &reply));
        let classification = match &result {
            Ok(()) => "accepted",
            Err(error) => error
                .downcast_ref::<super::wire::ResponseFailure>()
                .map_or("invalid_or_error", super::wire::ResponseFailure::outcome),
        };
        tracing::debug!(target: "piair::rtsp", phase = "feedback", status,
            classification, elapsed_us = started.elapsed().as_micros() as u64,
            "feedback outcome; receiver acceptance of audio unverified");
        result
    }
    fn validate_feedback(stream_id: Option<u32>, reply: &Reply) -> Result<()> {
        if !reply.body.is_empty() {
            let value = Self::plist_response(reply)?;
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
                        stream_id.map(u64::from) == Some(profile::integer(d, "streamID")?),
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
    fn finish(&mut self, result: Result<()>) -> Result<()> {
        if let Err(error) = &result {
            if self.failure.is_none() {
                self.failure = Some(format!("{error:#}"));
            }
            self.control.take();
            self.state = State::Failed;
        }
        result
    }
    #[cfg(test)]
    pub fn state(&self) -> State {
        self.state
    }
    #[cfg(test)]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
    pub fn stop(&mut self) -> Result<()> {
        self.control.take();
        if self.state != State::Failed {
            self.state = State::Disconnected;
        }
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

// While RTP runs with feedback enabled, this worker is the sole owner of Wire, its CSeq and
// directional cipher counters. The scheduler only uses bounded channels.
pub(super) struct FeedbackWorker {
    commands: Option<SyncSender<()>>,
    outcomes: Receiver<Result<()>>,
    thread: Option<JoinHandle<(Wire, Option<String>)>>,
    pending: bool,
}
impl FeedbackWorker {
    pub(super) fn start_bound(
        mut wire: Wire,
        mut session_header: Option<String>,
        stream_id: Option<u32>,
        cancel: Arc<AtomicBool>,
        strict: bool,
    ) -> Result<Self> {
        let (commands, requests) = mpsc::sync_channel(1);
        let (outcomes, responses) = mpsc::sync_channel(1);
        let dispatcher = tracing::dispatcher::get_default(Clone::clone);
        let thread = thread::Builder::new()
            .name("piair-feedback".into())
            .spawn(move || {
                tracing::dispatcher::with_default(&dispatcher, || {
                    wire.feedback_budget = Some(Duration::from_millis(500));
                    let previous_cancel = wire.diagnostic_cancel.replace(cancel.clone());
                    while requests.recv().is_ok() {
                        let started = Instant::now();
                        let result = (|| {
                            ensure!(!cancel.load(Ordering::Relaxed), "diagnostic interrupted");
                            let headers: Vec<_> = session_header
                                .iter()
                                .map(|token| ("Session", token.clone()))
                                .collect();
                            let reply = wire
                                .request("POST", "/feedback", &headers, &[])
                                .context("RTSP method=POST")?;
                            if strict {
                                if let Some(token) = reply.headers.get("session") {
                                    ensure!(
                                        session_header.as_ref() == Some(token),
                                        "receiver session binding changed"
                                    );
                                }
                            }
                            Session::validate_session_header(&mut session_header, reply)
                        })();
                        let result = Session::feedback_result(result, stream_id, started);
                        let failed = result.is_err();
                        if outcomes.send(result).is_err() || failed {
                            break;
                        }
                    }
                    wire.feedback_budget = None;
                    wire.diagnostic_cancel = previous_cancel;
                    (wire, session_header)
                })
            })
            .context("start feedback worker")?;
        Ok(Self {
            commands: Some(commands),
            outcomes: responses,
            thread: Some(thread),
            pending: false,
        })
    }
    pub(super) fn check(&mut self) -> Result<()> {
        if !self.pending {
            ensure!(
                !self
                    .thread
                    .as_ref()
                    .is_some_and(|thread| thread.is_finished()),
                "feedback worker stopped unexpectedly"
            );
        }
        if self.pending {
            match self.outcomes.try_recv() {
                Ok(result) => {
                    self.pending = false;
                    result?;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    bail!("feedback worker stopped without an outcome")
                }
            }
        }
        Ok(())
    }
    pub(super) fn request(&mut self) -> Result<()> {
        self.check()?;
        ensure!(!self.pending, "previous feedback request still pending");
        self.commands
            .as_ref()
            .context("feedback worker stopped")?
            .try_send(())
            .context("submit feedback request")?;
        self.pending = true;
        Ok(())
    }
    pub(super) fn reclaim(mut self) -> (Option<Wire>, Option<String>, Result<()>) {
        self.join()
    }
    fn join(&mut self) -> (Option<Wire>, Option<String>, Result<()>) {
        self.commands.take(); // No queued future polls or concurrent TEARDOWN.
        match self.thread.take().map(JoinHandle::join) {
            Some(Ok((wire, session_header))) => (Some(wire), session_header, self.check()),
            Some(Err(_)) => (None, None, Err(anyhow::anyhow!("feedback worker panicked"))),
            None => (None, None, Ok(())),
        }
    }
}
impl Drop for FeedbackWorker {
    fn drop(&mut self) {
        // Also join on unwinding; never detach a thread that owns control keys.
        let _ = self.join();
    }
}

#[cfg(test)]
mod feedback_tests {
    use super::*;
    fn mock_wire() -> (Wire, std::net::TcpStream) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let peer = listener.accept().unwrap().0;
        (Wire::mock(socket, Duration::from_millis(500), "test"), peer)
    }
    #[test]
    fn pending_feedback_is_nonblocking_bounded_and_late_errors_are_not_lost() {
        for failed in [false, true] {
            let (wire, _peer) = mock_wire();
            let (commands, requests) = mpsc::sync_channel(1);
            let (outcomes, responses) = mpsc::sync_channel(1);
            let (started, ready) = mpsc::sync_channel(0);
            let (release, gate) = mpsc::sync_channel(1);
            let thread = thread::spawn(move || {
                requests.recv().unwrap();
                started.send(()).unwrap();
                gate.recv_timeout(Duration::from_secs(1)).unwrap();
                outcomes
                    .send(if failed {
                        Err(anyhow::anyhow!("late invalid feedback"))
                    } else {
                        Ok(())
                    })
                    .unwrap();
                // Reclaim must close the command channel; no second request queues.
                assert!(matches!(
                    requests.recv_timeout(Duration::from_secs(1)),
                    Err(mpsc::RecvTimeoutError::Disconnected)
                ));
                (wire, Some("pinned-session".into()))
            });
            let mut worker = FeedbackWorker {
                commands: Some(commands),
                outcomes: responses,
                thread: Some(thread),
                pending: false,
            };
            worker.request().unwrap();
            ready.recv_timeout(Duration::from_secs(1)).unwrap();
            for _ in 0..100 {
                worker.check().unwrap();
            }
            assert!(worker.pending);
            assert!(worker.request().is_err());
            release.send(()).unwrap();
            // Simulate RTP ending before the pending response is observed.
            let (wire, session, result) = worker.reclaim();
            assert!(wire.is_some());
            assert_eq!(session.as_deref(), Some("pinned-session"));
            assert_eq!(result.is_err(), failed);
            if let Err(error) = result {
                assert!(error.to_string().contains("late invalid feedback"));
            }
        }
    }
    #[test]
    fn idle_worker_drop_joins_and_worker_panics_propagate() {
        let (wire, _peer) = mock_wire();
        let (commands, requests) = mpsc::sync_channel(1);
        let (_outcomes, responses) = mpsc::sync_channel(1);
        let joined = Arc::new(AtomicBool::new(false));
        let done = joined.clone();
        let thread = thread::spawn(move || {
            assert!(requests.recv().is_err());
            done.store(true, Ordering::Relaxed);
            (wire, None)
        });
        drop(FeedbackWorker {
            commands: Some(commands),
            outcomes: responses,
            thread: Some(thread),
            pending: false,
        });
        assert!(joined.load(Ordering::Relaxed));

        let (commands, _requests) = mpsc::sync_channel(1);
        let (_outcomes, responses) = mpsc::sync_channel(1);
        let worker = FeedbackWorker {
            commands: Some(commands),
            outcomes: responses,
            thread: Some(thread::spawn(|| panic!("test feedback worker panic"))),
            pending: true,
        };
        let (wire, _, result) = worker.reclaim();
        assert!(wire.is_none());
        assert!(result.unwrap_err().to_string().contains("worker panicked"));
    }
    #[test]
    fn feedback_validation_preserves_stream_identity_and_type() {
        for (id, kind, accepted) in [(7, 96, true), (8, 96, false), (7, 103, false)] {
            let value = profile::dict([(
                "streams",
                Value::Array(vec![profile::dict([
                    ("streamID", profile::uint(id)),
                    ("type", profile::uint(kind)),
                ])]),
            )]);
            let mut body = Vec::new();
            value.to_writer_binary(&mut body).unwrap();
            let reply = Reply {
                body: body.into(),
                headers: [(
                    "content-type".into(),
                    "application/x-apple-binary-plist".into(),
                )]
                .into(),
            };
            assert_eq!(
                Session::validate_feedback(Some(7), &reply).is_ok(),
                accepted
            );
        }
    }
}

#[cfg(test)]
mod ptp_handoff_tests {
    use super::*;
    #[test]
    fn handoff_moves_authenticated_wire_without_io_and_cancellation_closes_it() {
        use crate::receiver::crypto::ControlKeys;
        use std::io::Read;
        use std::net::{TcpListener, TcpStream};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut receiver, _) = listener.accept().unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut wire = Wire::mock(socket, Duration::from_secs(1), "device:112233445566")
            .secure(
                ControlKeys::new(b"handoff"),
                ControlKeys::events(b"handoff"),
            )
            .unwrap();
        let _approval = wire.ptp_test_authorization().unwrap();
        let mut session = Session::new();
        session.control = Some(wire);
        session.state = State::Negotiating;
        let cancel = session.cancel.clone();
        let mut ptp = session.into_ptp_foundation().unwrap();
        assert_eq!(ptp.state(), super::super::lifecycle::State::Negotiation);
        assert!(Arc::ptr_eq(&cancel, &ptp.cancel));
        cancel.store(true, Ordering::Relaxed);
        assert!(ptp.setup_authenticated().is_err());
        assert_eq!(ptp.state(), super::super::lifecycle::State::Failed);
        assert_eq!(receiver.read(&mut [0; 1]).unwrap(), 0);
    }
    #[test]
    fn ptp_authentication_preserves_preexisting_cancellation_without_dialing() {
        let mut session = Session::new();
        session.cancel.store(true, Ordering::Relaxed);
        assert!(session
            .authenticate_ptp_selected(|| panic!("cancelled session cannot dial"), None)
            .is_err());
        assert!(session.cancel.load(Ordering::Relaxed));
    }
    #[test]
    fn handoff_rejects_terminal_and_cancelled_owners() {
        let mut session = Session::new();
        for state in [
            State::Disconnected,
            State::Connecting,
            State::Authenticating,
            State::Failed,
        ] {
            session.state = state;
            assert!(session.check_ptp_handoff().is_err());
        }
        session.state = State::Negotiating;
        assert!(session.check_ptp_handoff().is_ok());
        session.cancel.store(true, Ordering::Relaxed);
        assert!(session.check_ptp_handoff().is_err());
        // A fabricated state cannot replace real authenticated Wire authorization.
        session.cancel.store(false, Ordering::Relaxed);
        assert!(session.into_ptp_foundation().is_err());
    }
}
