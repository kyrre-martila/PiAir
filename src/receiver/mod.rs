//! Explicit receiver operations. Discovery/audio never depend on this module.
pub(crate) mod compatibility;
mod crypto;
mod diagnostics;
mod hex_u64;
// Sans-I/O components only; blocked by the documented unresolved transport profile.
#[allow(dead_code)]
mod media;
mod pairing;
mod profile;
mod realtime;
mod session;
mod srp;
mod store;
#[cfg(test)]
mod tests;
mod tlv;
mod wire;
pub use wire::AuthProfile;

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
    /// Compare discovery with an approval without changing that approval. Exact
    /// equality remains intentional for persisted-selection/revocation checks.
    fn matches_observation(&self, observed: &Self) -> bool {
        self.id == observed.id
            && self.advertised_pi == observed.advertised_pi
            && self.method == observed.method
            && self.addresses == observed.addresses
            && self.port == observed.port
            && observed.capabilities.matches_selection(&self.capabilities)
    }
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
pub fn pair(
    loaded: &Loaded,
    id: &str,
    authenticate: bool,
    control_check: bool,
    auth_profile: AuthProfile,
) -> Result<()> {
    exact_id(id)?;
    ensure!(loaded.config.allows(id), "receiver is not approved");
    ensure!(
        (auth_profile != AuthProfile::Legacy)
            || (io::stdin().is_terminal() && io::stdout().is_terminal()),
        "pair/authenticate requires an interactive terminal"
    );
    let store = store::Store::new(&loaded.path)?;
    let selected = store
        .selection()?
        .context("select this approved receiver first")?;
    ensure!(selected.id == id, "receiver is not selected");
    let transient = selected.method == Method::Transient;
    ensure!(
        auth_profile == AuthProfile::Legacy || (authenticate && transient),
        "experimental profile requires authenticate with explicit transient selection; no fallback"
    );
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "pair/authenticate requires an interactive terminal"
    );
    let existing = if transient {
        None
    } else {
        store.credential(id)?
    };
    ensure!(
        !transient || authenticate,
        "transient selection does not enroll credentials; use authenticate or stream-test"
    );
    ensure!(
        transient || authenticate == existing.is_some(),
        "pair requires no existing credentials; authenticate requires stored credentials"
    );
    match auth_profile {
        AuthProfile::Legacy => (),
        AuthProfile::OwnToneTransient => println!("Controlled hardware experiment: OwnTone-reference RTSP transient authentication; HomePod compatibility unverified. No playback."),
        AuthProfile::PyatvDiagnostic => println!("Controlled hardware experiment: pyatv transient HTTP profile (preamble, M1, M3); no compatibility or HomePod support claim."),
    }
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
    if authenticate {
        let mut session = session::Session::new();
        session.start_selected(
            || {
                let mut wire = wire::Wire::connect(&loaded.path, id, &selected, &snapshot)?;
                match auth_profile {
                    AuthProfile::Legacy => (),
                    AuthProfile::OwnToneTransient => {
                        wire.owntone_transient(&selected.capabilities)?
                    }
                    AuthProfile::PyatvDiagnostic => wire.pyatv_transient_preamble()?,
                }
                Ok(wire)
            },
            if transient { None } else { existing.as_ref() },
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
        session
            .stop()
            .context("authentication session cleanup failed")?;
        if transient {
            println!("Authenticated {id} using transient SRP; no persistent receiver identity verified or credentials saved. Connection closed. No audio session.");
        } else {
            println!("Authenticated {id} using pair-verify; connection closed. No audio session.");
        }
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

/// Deliberate diagnostic only. No application startup/activity path calls this.
pub fn stream_test(loaded: &Loaded, id: &str, seconds: u32) -> Result<()> {
    exact_id(id)?;
    ensure!(loaded.config.allows(id), "receiver is not approved");
    ensure!(
        (1..=3).contains(&seconds),
        "test duration must be 1..3 seconds"
    );
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "stream-test requires an interactive terminal and explicit confirmation"
    );
    let store = store::Store::new(&loaded.path)?;
    let selected = store
        .selection()?
        .context("select this approved receiver first")?;
    ensure!(selected.id == id, "receiver is not selected");
    ensure!(
        selected.capabilities.features & (1 << 9) != 0
            && selected.capabilities.features & (1 << 19) != 0,
        "selected receiver does not advertise audio and ALAC"
    );
    let credential = if selected.method == Method::Transient {
        None
    } else {
        let credential = store
            .credential(id)?
            .context("pair this receiver explicitly first")?;
        ensure!(
            credential.advertised_pi == selected.advertised_pi,
            "credential discovery binding differs from selection"
        );
        Some(credential)
    };
    println!("AirPlay 2 realtime UDP / ALAC 44100 stereo / NTP diagnostic: {seconds}s signal plus 2s latency drain. Receiver volume will be set to -30 dB and may remain there. Hardware playback is unverified.");
    print!("Send test audio to {id}? Type the exact receiver ID: ");
    io::stdout().flush()?;
    let mut confirmation = String::new();
    io::stdin().read_line(&mut confirmation)?;
    ensure!(confirmation.trim() == id, "receiver operation cancelled");
    let snapshot = discovery::discover(loaded.config.discovery_timeout_secs)?;
    let mut session = session::Session::new();
    let cancel = session.cancel.clone();
    ctrlc::set_handler(move || cancel.store(true, std::sync::atomic::Ordering::Relaxed))?;
    session.start_selected(
        || wire::Wire::connect(&loaded.path, id, &selected, &snapshot),
        credential.as_ref(),
    )?;
    if selected.method == Method::Transient {
        println!("Authentication: transient SRP proof verified; no persistent receiver identity verification");
    } else {
        println!("Authentication: persistent receiver identity cryptographically verified");
    }
    if let Err(error) = session.negotiate_realtime() {
        println!("Negotiation: FAILED; audio packets transmitted: 0; partial setup cleanup attempted; audible playback: unverified");
        return Err(error);
    }
    println!("Negotiation: READY (SETUP, event channel, RECORD, stream SETUP and NTP exchange succeeded)");
    if let Err(error) = session.transmit_test(seconds) {
        println!("Transmission: FAILED after {} packets; cleanup attempted; audible playback: unverified",session.packets_sent);
        return Err(error);
    }
    println!(
        "Transmission: {} audio packets, {} retransmissions, {} timing replies",
        session.packets_sent, session.resends, session.timing_exchanges
    );
    session.stop()?;
    println!("Teardown: FLUSH and TEARDOWN acknowledged; resources closed. Audible playback: unverified (requires physical confirmation).");
    Ok(())
}
