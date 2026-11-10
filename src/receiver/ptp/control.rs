//! Typed requests on one borrowed authenticated Wire; no receiver dialing.
use super::{time::SampleTimeline, Grandmaster, State};
use crate::receiver::{buffered::PtpAnchor, profile, wire::Wire};
use anyhow::{ensure, Result};
use plist::Value;
use std::{net::IpAddr, time::Instant};
use zeroize::Zeroizing;

/// Only this receiver and the local address of its authenticated control socket.
/// Group addresses and arbitrary discovered peers cannot enter this request.
pub struct Peers {
    receiver: IpAddr,
    local: IpAddr,
}
impl Peers {
    pub(super) fn new(receiver: IpAddr, local: IpAddr) -> Result<Self> {
        for a in [receiver, local] {
            ensure!(
                !a.is_unspecified() && !a.is_multicast(),
                "invalid timing peer address"
            );
        }
        ensure!(
            receiver != local && receiver.is_ipv4() == local.is_ipv4(),
            "inconsistent timing peers"
        );
        Ok(Self { receiver, local })
    }
    pub fn encode(&self) -> Result<Zeroizing<Vec<u8>>> {
        profile::binary(&mut Value::Array(vec![
            Value::String(self.receiver.to_string()),
            Value::String(self.local.to_string()),
        ]))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rate {
    Pause,
    Play,
}
impl Rate {
    fn value(self) -> u64 {
        match self {
            Self::Pause => 0,
            Self::Play => 1,
        }
    }
}
pub struct AnchorRequest {
    anchor: PtpAnchor,
    rate: Rate,
    generation: u64,
}
impl AnchorRequest {
    pub fn generate(
        clock: &mut Grandmaster,
        mapping: &SampleTimeline,
        sample: u64,
        rate: Rate,
        now: Instant,
    ) -> Result<Self> {
        clock.check(now, false)?;
        ensure!(
            clock.state() == State::PeerObserved,
            "no valid receiver timing evidence"
        );
        ensure!(
            mapping.clock_id == clock.clock_id && mapping.generation == clock.generation(),
            "stale timeline mapping"
        );
        let time = mapping.at(sample)?;
        let current = clock.network_time(now)?;
        // Bound scheduling horizon; compare full Q64 time, not truncated seconds.
        let t = (u128::from(time.seconds) << 64) | u128::from(time.fraction);
        let n = (u128::from(current.seconds) << 64) | u128::from(current.fraction);
        ensure!(
            t >= n && t - n <= 10u128 << 64,
            "anchor outside scheduling window"
        );
        Ok(Self {
            anchor: PtpAnchor {
                clock_id: mapping.clock_id,
                seconds: time.seconds,
                fraction: time.fraction,
                sample_time: sample as u32,
            },
            rate,
            generation: mapping.generation,
        })
    }
    fn validate(&self, clock: &Grandmaster, now: Instant) -> Result<()> {
        ensure!(
            clock.state() == State::PeerObserved
                && self.anchor.clock_id == clock.clock_id
                && self.generation == clock.generation(),
            "stale timing anchor"
        );
        let current = clock.network_time(now)?;
        let time = (u128::from(self.anchor.seconds) << 64) | u128::from(self.anchor.fraction);
        let current = (u128::from(current.seconds) << 64) | u128::from(current.fraction);
        ensure!(
            time >= current && time - current <= 10u128 << 64,
            "expired timing anchor"
        );
        Ok(())
    }
    fn value(&self) -> Value {
        profile::dict([
            ("networkTimeSecs", profile::uint(self.anchor.seconds)),
            // Signed bplist integers preserve all 64 bits in an 8-byte object.
            (
                "networkTimeFrac",
                Value::Integer((self.anchor.fraction as i64).into()),
            ),
            (
                "networkTimeTimelineID",
                Value::Integer((self.anchor.clock_id as i64).into()),
            ),
            ("networkTimeFlags", profile::uint(0)),
            ("rtpTime", profile::uint(u64::from(self.anchor.sample_time))),
            ("rate", profile::uint(self.rate.value())),
        ])
    }
    pub fn encode(&self) -> Result<Zeroizing<Vec<u8>>> {
        profile::binary(&mut self.value())
    }
}
/// Wire validates RTSP 200 and CSeq first. Empty body is a known acknowledgement.
/// Fail closed on unknown body semantics, nonzero status or contradictory echoes.
pub fn acknowledgement(body: &[u8], expected: Option<&Value>) -> Result<()> {
    if body.is_empty() {
        return Ok(());
    }
    let value = profile::parse(body)?;
    let dict = profile::dictionary(&value)?;
    for (key, value) in dict {
        if key == "status" {
            ensure!(
                value.as_signed_integer() == Some(0),
                "timing control rejected"
            );
        } else {
            ensure!(
                expected
                    .and_then(Value::as_dictionary)
                    .and_then(|d| d.get(key))
                    == Some(value),
                "unsupported or contradictory timing acknowledgement"
            );
        }
    }
    Ok(())
}

fn validate_reply(reply: &crate::receiver::wire::Reply, expected: Option<&Value>) -> Result<()> {
    if !reply.body.is_empty() {
        if let Some(content_type) = reply.headers.get("content-type") {
            ensure!(
                content_type == "application/x-apple-binary-plist",
                "unsupported timing response content type"
            );
        }
    }
    acknowledgement(&reply.body, expected)
}

/// The exclusive Wire borrow prevents rebinding these operations to another
/// receiver/session. This owner does not implement the full BufferedLifecycle.
pub struct Control<'a> {
    wire: &'a mut Wire,
    uri: String,
    peers: Peers,
    lease: crate::receiver::wire::PtpLease,
    clock_id: u64,
    generation: u64,
    peers_accepted: bool,
    failed: bool,
}
impl<'a> Control<'a> {
    pub(crate) fn new(wire: &'a mut Wire, uri: String, clock: &Grandmaster) -> Result<Self> {
        let lease = wire.ptp_lease()?;
        let peers = Peers::new(lease.peer(), wire.local_ip()?)?;
        let clock_id = clock.clock_id;
        ensure!(
            clock_id != 0 && clock_id != u64::MAX,
            "invalid timing clock ID"
        );
        Ok(Self {
            wire,
            uri,
            peers,
            lease,
            clock_id,
            generation: clock.generation(),
            peers_accepted: false,
            failed: false,
        })
    }
    pub fn set_peers(&mut self) -> Result<()> {
        ensure!(
            !self.failed && !self.peers_accepted,
            "timing control already used/failed"
        );
        let result = (|| {
            self.lease.check()?;
            let body = self.peers.encode()?;
            let reply = self.wire.request(
                "SETPEERS",
                &self.uri,
                &[("Content-Type", "/peer-list-changed".into())],
                &body,
            )?;
            validate_reply(&reply, None)
        })();
        if result.is_err() {
            self.failed = true;
            self.wire.close();
        } else {
            self.peers_accepted = true;
        }
        result
    }
    pub fn set_rate_anchor_time(
        &mut self,
        clock: &mut Grandmaster,
        anchor: AnchorRequest,
        now: Instant,
    ) -> Result<()> {
        ensure!(
            !self.failed && self.peers_accepted,
            "SETPEERS acknowledgement required"
        );
        let result = (|| {
            self.lease.check()?;
            clock.check(now, false)?;
            ensure!(
                clock.state() == State::PeerObserved
                    && clock.clock_id == self.clock_id
                    && anchor.anchor.clock_id == self.clock_id
                    && anchor.generation == clock.generation()
                    && self.generation == clock.generation(),
                "anchor clock/session changed"
            );
            anchor.validate(clock, now)?;
            let body = anchor.encode()?;
            let reply = self.wire.request(
                "SETRATEANCHORTIME",
                &self.uri,
                &[("Content-Type", "application/x-apple-binary-plist".into())],
                &body,
            )?;
            clock.check(Instant::now(), false)?;
            validate_reply(&reply, Some(&anchor.value()))
        })();
        if result.is_err() {
            self.failed = true;
            self.wire.close();
        }
        result
    }
}
