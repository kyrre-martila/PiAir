//! Explicit receiver operations. Discovery/audio never depend on this module.
pub(crate) mod compatibility;
mod crypto;
mod pairing;
mod session;
mod srp;
mod store;
#[cfg(test)]
mod tests;
mod tlv;
mod wire;

use crate::{
    config::{self, Loaded},
    discovery::{self, Receiver},
};
use anyhow::{ensure, Context, Result};
use compatibility::{Capabilities, Method};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::{self, IsTerminal, Write},
    net::IpAddr,
};
use zeroize::Zeroizing;

pub fn exact_id(id: &str) -> Result<()> {
    ensure!(
        id.strip_prefix("device:")
            .and_then(config::device_id)
            .as_deref()
            == Some(id),
        "use an exact stable device:<12 lowercase hex digits> identifier"
    );
    Ok(())
}
fn valid_pairing_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b':')
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    id: String,
    advertised_pi: Option<String>,
    method: Method,
    addresses: BTreeSet<IpAddr>,
    port: u16,
    capabilities: Capabilities,
}
impl Selection {
    fn from_receiver(r: &Receiver, method: Method) -> Result<Self> {
        exact_id(&r.id)?;
        let services: Vec<_> = r
            .services
            .iter()
            .filter(|s| s.service_type == "_airplay._tcp.local.")
            .collect();
        ensure!(services.len() == 1, "ambiguous or missing AirPlay endpoint");
        let service = services[0];
        ensure!(service.matches_id(&r.id), "conflicting receiver identity");
        let capabilities = Capabilities::parse(service)?;
        capabilities.decide(method)?;
        let advertised_pi = service.capabilities.get("pi").cloned();
        ensure!(
            advertised_pi.as_deref().is_none_or(valid_pairing_id),
            "malformed advertised pi; it is not an authenticated receiver identifier"
        );
        ensure!(
            service.port != 0
                && !service.addresses.is_empty()
                && service
                    .addresses
                    .iter()
                    .all(|ip| !ip.is_unspecified() && !ip.is_multicast()),
            "invalid receiver endpoint"
        );
        Ok(Self {
            id: r.id.clone(),
            advertised_pi,
            method,
            addresses: service.addresses.clone(),
            port: service.port,
            capabilities,
        })
    }
}
pub fn list(loaded: &Loaded, receivers: &[Receiver]) -> Result<()> {
    let store = store::Store::new(&loaded.path)?;
    let selected = store.selection()?;
    println!("Passive advertisements; compatibility unverified. No direct receiver connection.");
    for r in receivers {
        let paired = if r.id.starts_with("device:") {
            store.credential(&r.id)?.is_some()
        } else {
            false
        };
        println!("{} {:?} discovered=true approved={} selected={} paired={} authenticated=false connected=false",r.id,r.name,loaded.config.allows(&r.id),selected.as_ref().is_some_and(|s|s.id==r.id),paired);
        for method in [Method::PinDisplay, Method::SetupCode, Method::Transient] {
            match Selection::from_receiver(r, method) {
                Ok(s) => println!("  {method:?}: implemented candidate; advertised buffered audio={}; playback unverified",s.capabilities.buffered_audio_advertised()),
                Err(e) => println!("  {method:?}: unavailable ({e}); no connection attempted"),
            }
        }
    }
    Ok(())
}
pub fn select(loaded: &Loaded, id: &str, method: Method) -> Result<()> {
    exact_id(id)?;
    ensure!(
        loaded.config.allows(id),
        "receiver unapproved; explicitly add its stable ID to receiver_allowlist first"
    );
    let snapshot = discovery::discover(loaded.config.discovery_timeout_secs)?;
    let receiver = snapshot
        .iter()
        .find(|r| r.id == id)
        .context("approved receiver not discovered")?;
    let selected = Selection::from_receiver(receiver, method)?;
    ensure!(
        !snapshot
            .iter()
            .filter(|r| r.id != id)
            .flat_map(|r| &r.services)
            .any(|s| s.addresses.iter().any(|ip| selected.addresses.contains(ip))),
        "conflicting receiver identities"
    );
    let store = store::Store::new(&loaded.path)?;
    if let Some(c) = store.credential(id)? {
        ensure!(
            c.advertised_pi == selected.advertised_pi,
            "advertised pi changed; investigate before explicit unpair/re-enrollment"
        );
    }
    store.select(&selected)?;
    println!("Selected {id}; endpoint and pairing ID pinned. No connection or playback started.");
    Ok(())
}
pub fn unpair(loaded: &Loaded, id: &str) -> Result<()> {
    exact_id(id)?;
    store::Store::new(&loaded.path)?.remove(id)?;
    println!("Local credentials removed for {id}. Receiver-side pairing and allowlist unchanged; no connection made.");
    Ok(())
}
pub fn pair(loaded: &Loaded, id: &str, authenticate: bool, control_check: bool) -> Result<()> {
    exact_id(id)?;
    ensure!(loaded.config.allows(id), "receiver is not approved");
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "pair/authenticate requires an interactive terminal"
    );
    let store = store::Store::new(&loaded.path)?;
    let selected = store
        .selection()?
        .context("select this approved receiver first")?;
    ensure!(selected.id == id, "receiver is not selected");
    let existing = store.credential(id)?;
    ensure!(
        authenticate == existing.is_some(),
        "pair requires no existing credentials; authenticate requires stored credentials"
    );
    print!(
        "{} {id}? Type the exact receiver ID to continue: ",
        if authenticate { "Authenticate" } else { "Pair" }
    );
    io::stdout().flush()?;
    let mut confirmation = String::new();
    io::stdin().read_line(&mut confirmation)?;
    ensure!(confirmation.trim() == id, "receiver operation cancelled");
    if let Some(c) = &existing {
        ensure!(
            c.advertised_pi == selected.advertised_pi,
            "credential discovery binding differs from selection"
        );
    }
    let snapshot = discovery::discover(loaded.config.discovery_timeout_secs)?;
    if let Some(c) = existing {
        let mut session = session::Session::new();
        session.start(
            || wire::Wire::connect(&loaded.path, id, &selected, &snapshot),
            &c,
        )?;
        if control_check {
            if let Err(error) = session.check_control() {
                tracing::debug!(state = ?session.state(), reason = session.failure(), "control diagnostic failed");
                return Err(error);
            }
            println!(
                "Authenticated encrypted OPTIONS completed; buffered audio remains unsupported."
            );
        }
        session.shutdown();
        println!("Authenticated {id} using pair-verify; connection closed. No audio session.");
    } else {
        let wire = wire::Wire::connect(&loaded.path, id, &selected, &snapshot)?;
        let credential = pairing::setup(wire, id, selected.advertised_pi.as_deref(), || {
            Ok(Zeroizing::new(rpassword::prompt_password(
                if selected.method.pin_start() {
                    "Enter the PIN displayed by the selected receiver: "
                } else {
                    "Enter the existing HAP setup code from the selected receiver: "
                },
            )?))
        })?;
        save_verified_enrollment(&loaded.path, &selected, &credential)?;
        println!("Paired {id}; verified credentials saved. Connection closed; not currently authenticated or connected. Use authenticate explicitly.");
    }
    Ok(())
}

// Called only after cryptographic enrollment succeeds. Keeping persistence separate
// makes approval revocation and selection replacement during a handshake testable.
fn save_verified_enrollment(
    path: &std::path::Path,
    selected: &Selection,
    credential: &store::Credential,
) -> Result<()> {
    let result = (|| -> Result<()> {
        ensure!(
            credential.receiver_id == selected.id
                && credential.advertised_pi == selected.advertised_pi,
            "verified credential differs from selected receiver"
        );
        ensure!(
            config::load(Some(path.to_owned()))?
                .config
                .allows(&selected.id),
            "receiver approval revoked during pairing"
        );
        let store = store::Store::new(path)?;
        ensure!(
            store.selection()?.as_ref() == Some(selected),
            "selection changed during pairing"
        );
        store.save(credential)
    })();
    result.context("local enrollment persistence did not complete; inspect local credentials and receiver-side controller records before an explicit retry")
}
