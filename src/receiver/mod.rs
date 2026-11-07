//! Explicit receiver operations. Discovery/audio never depend on this module.
mod crypto;
mod pairing;
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

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    id: String,
    pairing_id: String,
    addresses: BTreeSet<IpAddr>,
    port: u16,
    features: u64,
}
impl Selection {
    fn from_receiver(r: &Receiver) -> Result<Self> {
        exact_id(&r.id)?;
        let services: Vec<_> = r
            .services
            .iter()
            .filter(|s| s.service_type == "_airplay._tcp.local.")
            .collect();
        ensure!(services.len() == 1, "ambiguous or missing AirPlay endpoint");
        let service = services[0];
        ensure!(service.matches_id(&r.id), "conflicting receiver identity");
        let features = service
            .features()
            .context("missing/malformed AirPlay feature bits")?;
        // No method inference from friendly names/model/IP. HK access control is
        // the only persistent profile we implement; MFi-required receivers fail.
        ensure!(features & (1u64<<46)!=0 && features & (1u64<<51)==0,"unsupported pairing capabilities: requires persistent HK pairing without MFi; no fallback");
        let pairing_id = service
            .capabilities
            .get("pi")
            .context("receiver lacks pairing identifier pi; cannot bind signed identity safely")?
            .clone();
        ensure!(
            valid_pairing_id(&pairing_id),
            "invalid advertised pairing identity"
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
            pairing_id,
            addresses: service.addresses.clone(),
            port: service.port,
            features,
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
        for s in &r.services {
            println!(
                "  advertised AirPlay2 buffered audio={:?}; pairing profile supported={}",
                s.features().map(|f| f & (1u64 << 40) != 0),
                Selection::from_receiver(r).is_ok()
            );
        }
    }
    Ok(())
}
pub fn select(loaded: &Loaded, id: &str) -> Result<()> {
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
    let selected = Selection::from_receiver(receiver)?;
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
            c.pairing_id == selected.pairing_id,
            "receiver pairing identity changed; unpair explicitly after investigation"
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
pub fn pair(loaded: &Loaded, id: &str, authenticate: bool) -> Result<()> {
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
            c.pairing_id == selected.pairing_id,
            "credential identity differs from selection"
        );
    }
    let snapshot = discovery::discover(loaded.config.discovery_timeout_secs)?;
    let wire = wire::Wire::connect(&loaded.path, id, &selected, &snapshot)?;
    if let Some(c) = existing {
        let _keys = pairing::verify(wire, &c)?;
        println!("Authenticated {id} using pair-verify; connection closed. No audio session.");
    } else {
        let credential = pairing::setup(wire, id, &selected.pairing_id, || {
            Ok(Zeroizing::new(rpassword::prompt_password(
                "Enter the PIN displayed by the selected receiver: ",
            )?))
        })?;
        // Receiver cryptographic verification completed; recheck local approval
        // before persistence. Subsequent authentication requires a fresh authorized connection.
        ensure!(
            config::load(Some(loaded.path.clone()))?.config.allows(id),
            "receiver approval revoked during pairing"
        );
        ensure!(
            store.selection()?.as_ref() == Some(&selected),
            "selection changed during pairing"
        );
        store.save(&credential)?;
        println!("Paired {id}; verified credentials saved. Connection closed; not currently authenticated or connected. Use authenticate explicitly.");
    }
    Ok(())
}
