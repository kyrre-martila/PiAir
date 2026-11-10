//! Explicit receiver operations. Discovery/audio never depend on this module.
#[allow(dead_code)] // Reusable foundation; activation requires PR #42 authorized session lifecycle.
mod buffered;
pub(crate) mod compatibility;
mod crypto;
mod diagnostics;
mod hex_u64;
pub(crate) mod interactive;
pub(crate) mod lifecycle;
pub(crate) mod planning;
// Sans-I/O components only; blocked by the documented unresolved transport profile.
#[allow(dead_code)]
mod media;
mod pairing;
mod profile;
#[allow(dead_code)] // Playback timing primitives remain gated beyond the initial diagnostic.
mod ptp;
mod realtime;
mod session;
mod srp;
mod store;
#[cfg(test)]
mod tests;
mod tlv;
mod wire;
pub use session::SetupProfile;
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
    io::{self, IsTerminal},
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
    interactive::render_listing(loaded, receivers, &mut io::stdout().lock(), true)?;
    println!("Passive advertisements; identity, compatibility and playback unverified. Local credentials do not establish current authentication.");
    for r in receivers {
        println!("Compatibility candidates for {}:", r.id);
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
    select_from_snapshot(loaded, id, method, &snapshot)
}
fn select_from_snapshot(
    loaded: &Loaded,
    id: &str,
    method: Method,
    snapshot: &[Receiver],
) -> Result<()> {
    let selected = validate_selection(loaded, id, method, snapshot)?;
    let store = store::Store::new(&loaded.path)?;
    store.select(&selected)?;
    println!("Selected {id}; endpoint and pairing ID pinned. No connection or playback started.");
    Ok(())
}
// Shared by explicit select and provisional interactive selection; never persists.
fn validate_selection(
    loaded: &Loaded,
    id: &str,
    method: Method,
    snapshot: &[Receiver],
) -> Result<Selection> {
    exact_id(id)?;
    ensure!(loaded.config.allows(id), "receiver is not approved");
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
    Ok(selected)
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
    pair_with_selection(loaded, id, authenticate, control_check, auth_profile, None)
}
fn pair_with_selection(
    loaded: &Loaded,
    id: &str,
    authenticate: bool,
    control_check: bool,
    auth_profile: AuthProfile,
    proposed: Option<interactive::ProposedSelection>,
) -> Result<()> {
    exact_id(id)?;
    ensure!(loaded.config.allows(id), "receiver is not approved");
    ensure!(
        (auth_profile != AuthProfile::Legacy)
            || (io::stdin().is_terminal() && io::stdout().is_terminal()),
        "pair/authenticate requires an interactive terminal"
    );
    let store = store::Store::new(&loaded.path)?;
    let selected = match &proposed {
        Some(proposed) => proposed.selected.clone(),
        None => store
            .selection()?
            .context("select this approved receiver first")?,
    };
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
        AuthProfile::OwnToneTransient => println!("Controlled hardware experiment: OwnTone-reference RTSP transient authentication; receiver/firmware acceptance requires verification. No playback."),
        AuthProfile::PyatvDiagnostic => println!("Controlled hardware experiment: pyatv transient HTTP profile (preamble, M1, M3); no compatibility or HomePod support claim."),
    }
    if !authenticate {
        println!("Persistent enrollment requires receiver-supported PIN delivery for pin-display, or a genuine independently obtained HAP setup code for setup-code. An AirPlay password is not a HAP setup code. Do not invent codes or guess methods. If neither prerequisite is available, cancel: enrollment cannot currently proceed through these methods.");
    }
    if transient {
        println!("Transient authentication is session-scoped SRP without persistent receiver identity verification. No permanent pairing or credential enrollment. Authentication and encrypted control do not prove audible playback.");
    }
    let snapshot = discovery::discover(loaded.config.discovery_timeout_secs)?;
    interactive::describe(&snapshot, id)?;
    println!(
        "Operation: {} (no audio).",
        if authenticate {
            "Authenticate"
        } else {
            "Pair and save verified credentials"
        }
    );
    interactive::confirm_operation(loaded, proposed.as_ref(), &snapshot, "Continue? [y/N]: ")?;
    if let Some(c) = &existing {
        ensure!(
            c.advertised_pi == selected.advertised_pi,
            "credential discovery binding differs from selection"
        );
    }
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
#[allow(clippy::too_many_arguments)]
pub fn stream_test(
    loaded: &Loaded,
    id: Option<&str>,
    seconds: u32,
    volume_db: i32,
    setup_check: bool,
    record_check: bool,
    stream_setup_check: bool,
    playback_check: bool,
    setup_timeout_seconds: Option<u64>,
    setup_profile: Option<SetupProfile>,
    experimental_owntone_transient: bool,
) -> Result<()> {
    let stream_diagnostic = stream_setup_check || playback_check;
    ensure!(!stream_diagnostic || (!setup_check && !record_check
        && !(stream_setup_check && playback_check) && experimental_owntone_transient
        && id == Some(session::RECORD_RECEIVER_ID) && setup_profile.is_none()
        && setup_timeout_seconds.is_none() && seconds == 1 && volume_db == -30),
        "stream diagnostics require the exact approved HomePod, OwnTone transient, fixed defaults and no conflicting modes");
    ensure!(
        setup_profile.is_none() || setup_check,
        "profiles require --setup-check"
    );
    ensure!(
        setup_timeout_seconds.is_none() || (setup_check && setup_timeout_seconds == Some(15)),
        "SETUP timeout experiment requires --setup-check and 15 seconds"
    );
    ensure!(
        !experimental_owntone_transient || ((setup_check || record_check || stream_diagnostic) && id.is_some() && setup_profile.is_none()),
        "OwnTone transient requires an exact ID, an explicit diagnostic and no --setup-profile; no fallback"
    );
    ensure!(!record_check || (!setup_check && experimental_owntone_transient
        && id == Some(session::RECORD_RECEIVER_ID) && setup_profile.is_none()
        && setup_timeout_seconds.is_none()),
        "RECORD requires the approved HomePod exact ID, OwnTone transient and no other diagnostic profile");
    let proposed = if id.is_none() {
        Some(interactive::choose_for_stream(loaded)?)
    } else {
        None
    };
    let id = id.unwrap_or_else(|| &proposed.as_ref().unwrap().selected.id);
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
    let selected = match &proposed {
        Some(proposed) => proposed.selected.clone(),
        None => store
            .selection()?
            .context("select this approved receiver first")?,
    };
    ensure!(selected.id == id, "receiver is not selected");
    ensure!(
        selected.capabilities.features & (1 << 9) != 0
            && selected.capabilities.features & (1 << 19) != 0,
        "selected receiver does not advertise audio and ALAC"
    );
    if experimental_owntone_transient {
        ensure!(
            selected.method == Method::Transient,
            "OwnTone setup check requires explicit transient selection; no fallback"
        );
        selected.capabilities.decide(Method::Transient)?;
    }
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
    if stream_setup_check {
        println!("Stream SETUP-only check: authentication, initial SETUP, event connection, RECORD, stream SETUP, TEARDOWN. Zero audio; receiver volume unchanged. Successful negotiation does not prove audible playback.");
    } else if playback_check {
        println!("Playback check: one second original faded tone (peak below -18 dBFS), fixed receiver volume -40 dB, two second silent drain. Volume may persist and is NOT restored. No retries. Packet transmission does not prove audible playback.");
    } else if record_check {
        println!("RECORD-only OwnTone transient diagnostic: event channel and empty RECORD, then TEARDOWN. No audio or volume change. RECORD 200 does not prove stream negotiation or audible playback.");
    } else if experimental_owntone_transient {
        println!("Setup-only OwnTone-reference transient SRP selected; no persistent receiver identity verification; no fallback. Successful SETUP does not prove audible playback.");
    } else if selected.method == Method::Transient {
        println!("Stream-test uses the selected legacy transient SRP profile without persistent receiver identity verification. Experimental authenticate profiles are not used; no fallback. Successful authentication or transmission does not prove audible playback.");
    }
    if stream_setup_check || record_check {
        println!("Receiver volume unchanged; no playback operations.");
    } else if setup_check {
        println!("Initial NTP SETUP check: validate response and tear down; no RECORD or audio transmission. Receiver volume unchanged (--volume-db is playback-only).");
    } else {
        println!("The test includes a 2-second latency drain. Volume may persist after playback. Audible playback is unverified.");
    }
    ensure!(
        (-60..=-10).contains(&volume_db),
        "volume must be -60..=-10 dB"
    );
    let snapshot = discovery::discover(loaded.config.discovery_timeout_secs)?;
    interactive::describe(&snapshot, id)?;
    if stream_setup_check {
        println!("Operation: stream SETUP-only check");
    } else if playback_check {
        println!("Operation: playback check; confirm -40 dB receiver volume may persist; one second tone");
    } else if record_check {
        println!("Operation: RECORD-only check");
    } else if setup_check {
        println!("Operation: Setup-only check");
    } else {
        println!("Operation: Playback test\nDuration: {seconds} seconds\nVolume: {volume_db} dB");
    }
    let mut session = session::Session::new();
    interactive::confirm_operation(
        loaded,
        proposed.as_ref(),
        &snapshot,
        if stream_setup_check {
            "Start stream SETUP-only check? [y/N]: "
        } else if playback_check {
            "Authorize one second tone and persistent -40 dB receiver volume? [y/N]: "
        } else if record_check {
            "Start RECORD-only check? [y/N]: "
        } else if setup_check {
            "Start setup-only check? [y/N]: "
        } else {
            "Start playback test? [y/N]: "
        },
    )?;
    let cancel = session.cancel.clone();
    ctrlc::set_handler(move || cancel.store(true, std::sync::atomic::Ordering::Relaxed))?;
    let connect = || wire::Wire::connect(&loaded.path, id, &selected, &snapshot);
    if stream_diagnostic {
        session.start_owntone_stream_selected(connect, &selected.capabilities, playback_check)?;
    } else if record_check {
        session.start_owntone_record_selected(connect, &selected.capabilities)?;
    } else if experimental_owntone_transient {
        session.start_owntone_setup_selected(connect, &selected.capabilities)?;
    } else if setup_check {
        session.start_setup_selected(
            connect,
            credential.as_ref(),
            setup_profile.unwrap_or_default(),
        )?;
    } else {
        session.start_selected(connect, credential.as_ref())?;
    }
    if selected.method == Method::Transient {
        println!("Authentication: transient SRP proof verified; no persistent receiver identity verification");
    } else {
        println!("Authentication: persistent receiver identity cryptographically verified");
    }
    if stream_setup_check {
        session.check_stream()?;
        println!("Stream SETUP validated; TEARDOWN acknowledged; local cleanup complete; audio packets: 0; volume unchanged. Audible playback unverified.");
        return Ok(());
    }
    if playback_check {
        session.prepare_playback_check()?;
        session.transmit_test(1)?;
        session.stop()?;
        println!("Bounded playback transmission: {} packets; TEARDOWN acknowledged; local cleanup complete. Receiver volume -40 dB may persist; audible playback unverified.", session.packets_sent);
        return Ok(());
    }
    if record_check {
        let result = session.check_record();
        println!("RECORD validated: {}; audio packets transmitted: 0. Stream negotiation and audible playback unverified.", session.record_succeeded());
        result?;
        println!("TEARDOWN: acknowledged; local resources released.");
        return Ok(());
    }
    if setup_check {
        session.check_initial_setup_with_timeout(
            setup_timeout_seconds.map(std::time::Duration::from_secs),
        )?;
        println!("Initial SETUP: validated NTP response and ports; TEARDOWN acknowledged; audio packets transmitted: 0.");
        return Ok(());
    }
    if let Err(error) = session.negotiate_realtime_at_volume(volume_db) {
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

/// Deliberate Stua-only diagnostics; never invoked by capture or production startup.
pub fn stua_check(loaded: &Loaded, id: &str, playback: bool, volume_db: i32) -> Result<()> {
    ensure!(
        id == session::STUA_RECEIVER_ID,
        "Stua diagnostic requires device:02d899c1833f; no other receiver contact"
    );
    ensure!(loaded.config.allows(id), "receiver is not approved");
    let store = store::Store::new(&loaded.path)?;
    let credential = store
        .credential(id)?
        .context("existing Stua HAP credentials required; no automatic pairing")?;
    let method = credential.authentication_method(id)?;
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "Stua diagnostic requires an interactive terminal and explicit confirmation"
    );
    let snapshot = discovery::discover(loaded.config.discovery_timeout_secs)?;
    // Validate the exact endpoint before confirmation as well as at Wire::connect.
    let selected = validate_selection(loaded, id, method, &snapshot)?;
    stua_credential(&store, &selected)?;
    interactive::describe(&snapshot, id)?;
    println!("Persistent HAP pair-verify using existing stored keys; no pairing, credential writes, retries or receiver fallback. Only NTP realtime ALAC is supported; PTP/FairPlay/MFi requirements fail closed. Stua uses HomePod 2 as default output; routing and audible playback remain unverified.");
    let prompt = if playback {
        println!("One second original faded tone, receiver volume {volume_db} dB, two seconds silent drain. WARNING: receiver volume may persist after the test, including after failure; it is not restored. Packet transmission does not prove audible playback.");
        format!("Authorize Stua tone and persistent {volume_db} dB receiver volume? [y/N]: ")
    } else {
        println!("Stream SETUP-only: authentication, NTP SETUP, event connection, RECORD, stream SETUP and TEARDOWN. Zero audio, zero sync packets, no volume changes. Negotiation does not prove audible playback.");
        "Start Stua zero-audio stream SETUP-only check? [y/N]: ".to_owned()
    };
    interactive::confirm_operation(loaded, None, &snapshot, &prompt)?;
    let mut session = session::Session::new();
    let cancel = session.cancel.clone();
    ctrlc::set_handler(move || cancel.store(true, std::sync::atomic::Ordering::Relaxed))?;
    session.start_stua_stream_selected(
        || wire::Wire::connect_stua_operation(&loaded.path, &selected, &snapshot, &credential),
        &credential,
        playback,
    )?;
    if playback {
        session.prepare_playback_check_at_volume(volume_db)?;
        session.transmit_test(1)?;
        session.stop()?;
        println!("Stua: {} RTP packets transmitted; TEARDOWN acknowledged; local cleanup complete. Volume {volume_db} dB may persist. Audible playback unverified.", session.packets_sent);
    } else {
        session.check_stream()?;
        println!("Stua stream SETUP validated; TEARDOWN acknowledged; local cleanup complete. Audio packets: 0; volume unchanged. Audible playback unverified.");
    }
    Ok(())
}

fn stua_credential(store: &store::Store, selected: &Selection) -> Result<store::Credential> {
    ensure!(
        selected.id == session::STUA_RECEIVER_ID,
        "operation selection must be Stua; no selection changes or fallback"
    );
    ensure!(
        selected.method == Method::StoredHap,
        "Stua requires stored persistent HAP authentication; no method guessing or fallback"
    );
    selected.capabilities.decide(selected.method)?;
    ensure!(
        selected.capabilities.features & ((1 << 9) | (1 << 19)) == ((1 << 9) | (1 << 19)),
        "Stua must advertise audio and ALAC"
    );
    let credential = store
        .credential(&selected.id)?
        .context("existing Stua HAP credentials required; no automatic pairing")?;
    ensure!(
        credential.authentication_method(session::STUA_RECEIVER_ID)? == selected.method,
        "credential authentication method differs from operation"
    );
    ensure!(
        credential.advertised_pi == selected.advertised_pi,
        "credential discovery binding differs from selection"
    );
    Ok(credential)
}

/// Explicit buffered initial-SETUP diagnostic. Playback requests fail before discovery or dialing.
pub fn buffered_test(
    loaded: &Loaded,
    id: &str,
    setup_check: bool,
    confirm_audio: bool,
    control_check: Option<lifecycle::Stage>,
) -> Result<()> {
    exact_id(id)?;
    if confirm_audio || (!setup_check && control_check.is_none()) {
        anyhow::bail!(lifecycle::PLAYBACK_BLOCKER);
    }
    ensure!(loaded.config.allows(id), "receiver is not approved");
    let store = store::Store::new(&loaded.path)?;
    let selected = store
        .selection()?
        .context("select this approved receiver first")?;
    ensure!(selected.id == id, "receiver is not selected");
    selected.capabilities.decide(selected.method)?;
    ensure!(
        selected.capabilities.features & ((1u64 << 40) | (1u64 << 41))
            == ((1u64 << 40) | (1u64 << 41)),
        "selected receiver must advertise buffered audio and PTP; no profile fallback"
    );
    let credential = if selected.method == Method::Transient {
        None
    } else {
        let credential = store
            .credential(id)?
            .context("existing credentials required; no automatic pairing")?;
        ensure!(
            credential.authentication_method(id)? == selected.method
                && credential.advertised_pi == selected.advertised_pi,
            "credentials differ from selected authentication binding"
        );
        Some(credential)
    };
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "buffered diagnostic requires interactive confirmation"
    );
    let snapshot = discovery::discover(loaded.config.discovery_timeout_secs)?;
    println!("Buffered diagnostic: initial SETUP and one second of timing observation; any additional control stages need separate confirmation. Bounded TEARDOWN follows. No audio or volume changes. Clock adoption and audible playback remain unverified.");
    interactive::confirm_operation(
        loaded,
        None,
        &snapshot,
        "Contact the selected receiver for this initial SETUP check? [y/N]: ",
    )?;
    if let Some(target) = control_check {
        for (stage, prompt) in [
            (lifecycle::Stage::Record, "Opt in to RECORD on this session? [y/N]: "),
            (lifecycle::Stage::Stream, "Opt in to SETPEERS, empty MediaRemote registration, stream SETUP and audioMode (fresh secret key; no media connection)? [y/N]: "),
            (lifecycle::Stage::Anchor, "Opt in to a paused timing anchor (acceptance does not prove clock adoption)? [y/N]: "),
        ] {
            if stage <= target {
                interactive::confirm_operation(loaded, None, &snapshot, prompt)?;
            }
        }
    }
    let mut session = lifecycle::Session::new();
    let cancel = session.cancel.clone();
    ctrlc::set_handler(move || cancel.store(true, std::sync::atomic::Ordering::Relaxed))?;
    session.start(
        || wire::Wire::connect(&loaded.path, id, &selected, &snapshot),
        credential.as_ref(),
    )?;
    let observed = session.observe(std::time::Duration::from_secs(1))?;
    if let Some(stage) = control_check {
        session.progress(stage)?;
        println!(
            "Control stage {stage:?} accepted; receiver clock adoption unproven; audio bytes: 0."
        );
    }
    session.stop()?;
    println!(
        "PTP activity: {observed:?}; session: {:?}; TEARDOWN acknowledged. Audio bytes: 0. {}",
        session.state(),
        lifecycle::PLAYBACK_BLOCKER
    );
    Ok(())
}
