//! Typed requests on one borrowed authenticated Wire; no receiver dialing.
use crate::receiver::profile;
use anyhow::{ensure, Result};
use plist::Value;
use std::net::IpAddr;
use zeroize::Zeroizing;

/// Only this receiver and the local address of its authenticated control socket.
/// Group addresses and arbitrary discovered peers cannot enter this request.
pub struct Peers {
    receiver: IpAddr,
    local: IpAddr,
}
impl Peers {
    pub(crate) fn new(receiver: IpAddr, local: IpAddr) -> Result<Self> {
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
            let status = value
                .as_signed_integer()
                .ok_or_else(|| anyhow::anyhow!("invalid timing control status"))?;
            ensure!(
                status == 0,
                "timing control rejected in-band: status={status}"
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

pub(crate) fn validate_reply(
    reply: &crate::receiver::wire::Reply,
    expected: Option<&Value>,
) -> Result<()> {
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
