//! Persistent authenticated control owner. Audio negotiation is deliberately gated.
use super::{pairing, store::Credential, wire::Wire};
use anyhow::{bail, ensure, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Disconnected,
    Connecting,
    Authenticating,
    Negotiating,
    // Reserved until a verified buffered profile implements SETUP and RECORD.
    #[allow(dead_code)]
    Ready,
    #[allow(dead_code)]
    Streaming,
    Stopping,
    Failed,
}

pub struct Session {
    state: State,
    control: Option<Wire>,
    failure: Option<String>,
}
impl Session {
    pub fn new() -> Self {
        Self {
            state: State::Disconnected,
            control: None,
            failure: None,
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
            bail!("buffered audio unsupported: media key exchange, codec mapping, TCP framing and receiver clock/feedback profile are unverified; FairPlay is not implemented")
        })();
        self.finish(result)
    }
    fn finish(&mut self, result: Result<()>) -> Result<()> {
        if let Err(error) = &result {
            self.control = None;
            self.state = State::Failed;
            if self.failure.is_none() {
                self.failure = Some(format!("{error:#}"));
            }
        }
        result
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
    pub fn shutdown(&mut self) {
        if self.state == State::Disconnected {
            return;
        }
        let failed = self.state == State::Failed;
        self.state = State::Stopping;
        self.control = None;
        self.state = if failed {
            State::Failed
        } else {
            State::Disconnected
        };
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.shutdown();
    }
}
