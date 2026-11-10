//! Single-receiver buffered lifecycle foundation. Live media remains fail-closed.
//! No public way to assert receiver clock adoption exists. See the evidence ledger.
use super::{
    pairing,
    planning::{Claim, Evidence},
    profile, ptp,
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
    // Deliberately unreachable until an independently validated adoption protocol exists.
    #[allow(dead_code)]
    Prebuffering,
    #[allow(dead_code)]
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
                    | (
                        Self::Authorization
                            | Self::Authentication
                            | Self::Negotiation
                            | Self::Timing,
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

/// Owns the authenticated connection, PTP sockets and event worker. No background
/// timing thread, queued PCM, imported media key or reusable playback grant.
pub struct Session {
    state: State,
    control: Option<Wire>,
    timing: Option<ptp::Service>,
    events: Option<Worker>,
    uri: String,
    token: Option<String>,
    attempted_setup: bool,
    observed: bool,
    clock_id: u64,
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
            control: None,
            timing: None,
            events: None,
            uri: String::new(),
            token: None,
            attempted_setup: false,
            observed: false,
            clock_id,
            uuid: uuid::Uuid::new_v4(),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn state(&self) -> State {
        self.state
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
    fn call(
        &mut self,
        method: &str,
        path: Option<&str>,
        body: &[u8],
    ) -> Result<super::wire::Reply> {
        if method != "TEARDOWN" {
            self.check()?;
        }
        let mut headers = Vec::new();
        if !body.is_empty() {
            headers.push((
                "Content-Type",
                "application/x-apple-binary-plist".to_owned(),
            ));
        }
        if let Some(token) = &self.token {
            headers.push(("Session", token.clone()));
        }
        let reply = self.control.as_mut().context("control absent")?.request(
            method,
            path.unwrap_or(&self.uri),
            &headers,
            body,
        )?;
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
                self.token.as_ref().is_none_or(|old| old == token),
                "receiver session changed"
            );
            self.token = Some(token.clone());
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
            let timing = wire.start_ptp(clock_id, setup.clock_id, Duration::from_secs(2))?;
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
        validate_info(&info, wire.receiver_id())?;
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
        let response = Self::plist(&self.call("SETUP", None, &offer)?)?;
        let setup = InitialSetup::parse(&response, peer, self.clock_id)?;
        self.check()?;
        let wire = self.control.as_mut().context("control absent")?;
        let (events, timing) = resources(wire, setup, self.clock_id)?;
        self.events = events;
        self.timing = Some(timing);
        self.state.advance(State::Timing)
    }
    /// Timing is serviced on the owning thread. No RTSP request is made while
    /// it is active, so a blocking control read cannot starve the PTP service.
    /// Peer traffic is reported only as activity, never synchronization.
    pub fn observe(&mut self, duration: Duration) -> Result<ptp::State> {
        ensure!(
            self.state == State::Timing && !self.observed,
            "timing diagnostic not active or already observed"
        );
        self.observed = true;
        let result = (|| {
            ensure!(
                !duration.is_zero() && duration <= Duration::from_secs(1),
                "timing observation must be at most one second"
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
        // Stop PTP before bounded control cleanup. Neither timing nor events
        // can survive the owner; event Drop closes its socket and joins.
        self.timing.take();
        self.events.take();
        let result = (|| {
            if self.attempted_setup {
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
            }
            Ok(())
        })();
        if let Some(mut wire) = self.control.take() {
            wire.close();
        }
        self.token = None;
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

fn validate_info(value: &Value, receiver_id: &str) -> Result<()> {
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
    ensure!(
        features & ((1u64 << 40) | (1u64 << 41)) == ((1u64 << 40) | (1u64 << 41)),
        "buffered audio and PTP required; no fallback"
    );
    ensure!(
        matches!(evidence.buffered_formats, Claim::Observed(ref formats) if formats.contains(&21)),
        "authenticated ALAC24/48000/stereo format unavailable or contradictory"
    );
    // Conservative diagnostic policy. No password/FairPlay or policy guessing.
    ensure!(
        profile::integer(root, "statusFlags")? & ((1 << 3) | (1 << 7) | (1 << 9)) == 0,
        "unsupported authenticated access policy"
    );
    for field in ["acl", "act", "accessControlLevel", "accessControlType"] {
        if root.contains_key(field) {
            ensure!(
                profile::integer(root, field)? == 0,
                "unsupported authenticated access policy"
            );
        }
    }
    for field in ["isGroupLeader", "isGroupMember", "isProxyGroupPlayer"] {
        if let Some(v) = root.get(field) {
            ensure!(
                v.as_boolean() == Some(false),
                "group/proxy receiver unsupported"
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
    let peer = profile::dict([
        ("ID", Value::String(uuid.to_string().to_uppercase())),
        ("DeviceType", profile::uint(0)),
        ("ClockID", Value::Integer((clock_id as i64).into())),
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
            .context("receiver ClockID unavailable; cannot safely bind PTP")?;
        ensure!(
            clock != 0 && clock != u64::MAX && clock != local_clock,
            "invalid receiver clock identity"
        );
        let addresses = timing
            .get("Addresses")
            .and_then(Value::as_array)
            .context("timing addresses absent")?;
        ensure!(
            addresses.len() == 1
                && addresses[0]
                    .as_string()
                    .and_then(|v| v.parse::<IpAddr>().ok())
                    == Some(peer),
            "timing peer differs from authorized control peer; no address fallback"
        );
        Ok(Self {
            event_port: profile::port(root, "eventPort")?,
            clock_id: clock,
        })
    }
}

#[cfg(test)]
mod tests;
