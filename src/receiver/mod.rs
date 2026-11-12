//! Explicit receiver operations. Discovery/audio never depend on this module.
mod automatic;
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
mod playback;
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
mod wav;
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
    audio_file: Option<&std::path::Path>,
    setup_timeout_seconds: Option<u64>,
    setup_profile: Option<SetupProfile>,
    experimental_owntone_transient: bool,
) -> Result<()> {
    ensure!(
        audio_file.is_none()
            || (!setup_check
                && !record_check
                && !stream_setup_check
                && !playback_check
                && setup_profile.is_none()
                && setup_timeout_seconds.is_none()),
        "WAV playback cannot be combined with another diagnostic mode or SETUP options"
    );
    let wav = audio_file
        .map(|path| wav::Playback::open(path, seconds))
        .transpose()?;
    let stream_diagnostic = stream_setup_check || playback_check;
    ensure!(!stream_diagnostic || (!setup_check && !record_check
        && !(stream_setup_check && playback_check) && experimental_owntone_transient
        && setup_profile.is_none()
        && setup_timeout_seconds.is_none() && seconds == 1 && volume_db == -30),
        "stream diagnostics require an approved compatible receiver, OwnTone transient, fixed defaults and no conflicting modes");
    ensure!(
        setup_profile.is_none() || setup_check,
        "profiles require --setup-check"
    );
    ensure!(
        setup_timeout_seconds.is_none() || (setup_check && setup_timeout_seconds == Some(15)),
        "SETUP timeout experiment requires --setup-check and 15 seconds"
    );
    ensure!(
        !experimental_owntone_transient
            || ((setup_check || record_check || stream_diagnostic || wav.is_some())
                && setup_profile.is_none()),
        "OwnTone transient requires an explicit diagnostic and no --setup-profile; no fallback"
    );
    ensure!(!record_check || (!setup_check && experimental_owntone_transient
        && setup_profile.is_none()
        && setup_timeout_seconds.is_none()),
        "RECORD requires an approved compatible receiver, OwnTone transient and no other diagnostic profile");
    let proposed = if id.is_none() {
        Some(interactive::choose_for_stream(loaded)?)
    } else {
        None
    };
    let id = id.unwrap_or_else(|| &proposed.as_ref().unwrap().selected.id);
    exact_id(id)?;
    ensure!(loaded.config.allows(id), "receiver is not approved");
    ensure!(
        (1..=if wav.is_some() { 30 } else { 3 }).contains(&seconds),
        "test duration must be 1..3 seconds (WAV: 1..30)"
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
    let (selected, credential) = if wav.is_some() {
        wav_authentication(&store, &selected, experimental_owntone_transient)?
    } else if selected.method == Method::Transient {
        (selected, None)
    } else {
        let credential = store
            .credential(id)?
            .context("pair this receiver explicitly first")?;
        ensure!(
            credential.advertised_pi == selected.advertised_pi,
            "credential discovery binding differs from selection"
        );
        (selected, Some(credential))
    };
    if let Some(wav) = &wav {
        println!("WAV playback: {} frames at 44100 Hz stereo; {}. WARNING: receiver volume {volume_db} dB may persist after playback or failure and is not restored. RTP transmission does not prove audible playback.", wav.frames.len(), if wav.eof { "ends at EOF; no looping" } else { "stops at requested duration; remainder is not played" });
    } else if stream_setup_check {
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
    if wav.is_some() && selected.method == Method::StoredHap {
        let observed = validate_selection(loaded, id, Method::StoredHap, &snapshot)?;
        ensure!(
            selected.matches_observation(&observed),
            "receiver changed; review and select again explicitly"
        );
    }
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
        if wav.is_some() {
            "Authorize WAV audio and persistent receiver volume? [y/N]: "
        } else if stream_setup_check {
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
    if wav.is_some() && !experimental_owntone_transient {
        let credential = credential
            .as_ref()
            .context("stored HAP credentials required")?;
        session.start_hap_stream_selected(
            || wire::Wire::connect_hap_operation(&loaded.path, &selected, &snapshot, credential),
            credential,
            true,
        )?;
    } else if stream_diagnostic || wav.is_some() {
        session.start_owntone_stream_selected(
            connect,
            &selected.capabilities,
            playback_check || wav.is_some(),
        )?;
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
    if let Some(wav) = wav {
        session.prepare_playback_check_at_volume(volume_db)?;
        session.transmit_wav(&wav.frames)?;
        session.stop()?;
        println!("WAV transmission complete: {} RTP packets; TEARDOWN acknowledged; local cleanup complete. Receiver volume may persist. Audible playback unverified.", session.packets_sent);
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

/// Deliberate stored-HAP diagnostics; never invoked by capture or production startup.
pub fn hap_check(loaded: &Loaded, id: Option<&str>, playback: bool, volume_db: i32) -> Result<()> {
    let proposed = if id.is_none() {
        Some(interactive::choose_for_stream(loaded)?)
    } else {
        None
    };
    let id = id.unwrap_or_else(|| &proposed.as_ref().unwrap().selected.id);
    exact_id(id)?;
    ensure!(loaded.config.allows(id), "receiver is not approved");
    let store = store::Store::new(&loaded.path)?;
    let credential = store
        .credential(id)?
        .context("existing receiver HAP credentials required; no automatic pairing")?;
    let method = credential.authentication_method(id)?;
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "Receiver diagnostic requires an interactive terminal and explicit confirmation"
    );
    let snapshot = discovery::discover(loaded.config.discovery_timeout_secs)?;
    // Validate the exact endpoint before confirmation as well as at Wire::connect.
    let selected = validate_selection(loaded, id, method, &snapshot)?;
    if let Some(proposed) = &proposed {
        ensure!(
            proposed.selected.method != Method::Transient,
            "HAP diagnostic requires a persistent selection and existing credentials; no fallback"
        );
        // Persisted methods describe enrollment intent; StoredHap is deliberately
        // operation-only and derived from the existing validated credential.
        let mut operation = proposed.selected.clone();
        operation.method = method;
        ensure!(
            operation.matches_observation(&selected),
            "receiver changed; review and select again explicitly"
        );
    }
    hap_credential(&store, &selected)?;
    interactive::describe(&snapshot, id)?;
    println!("Persistent HAP pair-verify using existing stored keys; no pairing, credential writes, retries or receiver fallback. Only NTP realtime ALAC is supported; PTP/FairPlay/MFi requirements fail closed. Routing and audible playback remain unverified.");
    let prompt = if playback {
        println!("One second original faded tone, receiver volume {volume_db} dB, two seconds silent drain. WARNING: receiver volume may persist after the test, including after failure; it is not restored. Packet transmission does not prove audible playback.");
        format!("Authorize receiver tone and persistent {volume_db} dB receiver volume? [y/N]: ")
    } else {
        println!("Stream SETUP-only: authentication, NTP SETUP, event connection, RECORD, stream SETUP and TEARDOWN. Zero audio, zero sync packets, no volume changes. Negotiation does not prove audible playback.");
        "Start receiver zero-audio stream SETUP-only check? [y/N]: ".to_owned()
    };
    interactive::confirm_operation(loaded, proposed.as_ref(), &snapshot, &prompt)?;
    let mut session = session::Session::new();
    let cancel = session.cancel.clone();
    ctrlc::set_handler(move || cancel.store(true, std::sync::atomic::Ordering::Relaxed))?;
    session.start_hap_stream_selected(
        || wire::Wire::connect_hap_operation(&loaded.path, &selected, &snapshot, &credential),
        &credential,
        playback,
    )?;
    if playback {
        session.prepare_playback_check_at_volume(volume_db)?;
        session.transmit_test(1)?;
        session.stop()?;
        println!("Receiver: {} RTP packets transmitted; TEARDOWN acknowledged; local cleanup complete. Volume {volume_db} dB may persist. Audible playback unverified.", session.packets_sent);
    } else {
        session.check_stream()?;
        println!("Receiver stream SETUP validated; TEARDOWN acknowledged; local cleanup complete. Audio packets: 0; volume unchanged. Audible playback unverified.");
    }
    Ok(())
}

/// Derive an operation-only HAP method from a persistent selection and local keys.
/// Never reinterpret a transient selection or persist StoredHap as enrollment intent.
fn wav_authentication(
    store: &store::Store,
    selected: &Selection,
    transient: bool,
) -> Result<(Selection, Option<store::Credential>)> {
    let mut operation = selected.clone();
    if transient {
        ensure!(
            selected.method == Method::Transient,
            "--experimental-owntone-transient requires explicit transient selection; no fallback"
        );
        selected.capabilities.decide(Method::Transient)?;
        return Ok((operation, None));
    }
    ensure!(
        selected.method != Method::Transient,
        "WAV requires a persistent receiver selection and existing HAP credentials; select the paired receiver explicitly, or opt in to --experimental-owntone-transient where receiver policy permits; no fallback"
    );
    operation.method = Method::StoredHap;
    let credential = hap_credential(store, &operation)?;
    Ok((operation, Some(credential)))
}

fn hap_credential(store: &store::Store, selected: &Selection) -> Result<store::Credential> {
    ensure!(
        selected.method == Method::StoredHap,
        "Receiver requires stored persistent HAP authentication; no method guessing or fallback"
    );
    selected.capabilities.decide(selected.method)?;
    ensure!(
        selected.capabilities.features & ((1 << 9) | (1 << 19)) == ((1 << 9) | (1 << 19)),
        "Receiver must advertise audio and ALAC"
    );
    let credential = store
        .credential(&selected.id)?
        .context("existing receiver HAP credentials required; no automatic pairing")?;
    ensure!(
        credential.authentication_method(&selected.id)? == selected.method,
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
    if selected.method == Method::Transient {
        println!("Transient default: OwnTone-reference RTSP /info and SRP on the same encrypted control connection; no fallback, persistent receiver identity verification or pairing credentials saved.");
    }
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
    let result = (|| {
        session.start(
            || wire::Wire::connect(&loaded.path, id, &selected, &snapshot),
            credential.as_ref(),
        )?;
        session.observe(std::time::Duration::from_secs(1))?;
        if let Some(stage) = control_check {
            session.progress(stage)?;
        }
        session.stop()
    })();
    println!("Buffered diagnostic: {}", session.diagnostic());
    result?;
    Ok(())
}

/// Explicit selected-receiver diagnostic, sharing WAV authorization and authentication.
pub fn ptp_negotiation_check(
    loaded: &Loaded,
    id: &str,
    transient: bool,
    seconds: u64,
) -> Result<()> {
    ptp_operation(loaded, id, transient, seconds, None, None, None)
}

pub fn ptp_wav_playback(
    loaded: &Loaded,
    id: &str,
    path: &std::path::Path,
    seconds: u32,
    volume: Option<i32>,
    transient: bool,
) -> Result<()> {
    let wav = wav::Playback::open(path, seconds)?;
    ptp_operation(loaded, id, transient, 10, Some((wav, volume)), None, None)
}

pub fn ptp_capture(
    loaded: &Loaded,
    id: &str,
    device: &str,
    seconds: Option<u32>,
    volume: Option<i32>,
    transient: bool,
) -> Result<()> {
    ptp_operation(
        loaded,
        id,
        transient,
        10,
        None,
        Some((device, seconds, volume)),
        None,
    )
}

pub fn automatic_capture(
    loaded: &Loaded,
    id: &str,
    device: &str,
    volume: Option<i32>,
    transient: bool,
    settings: crate::audio::activity::Settings,
) -> Result<()> {
    ptp_operation(
        loaded,
        id,
        transient,
        10,
        None,
        Some((device, None, volume)),
        Some(settings.validate()?),
    )
}

#[allow(clippy::too_many_arguments)]
fn ptp_operation(
    loaded: &Loaded,
    id: &str,
    transient: bool,
    seconds: u64,
    playback: Option<(wav::Playback, Option<i32>)>,
    live: Option<(&str, Option<u32>, Option<i32>)>,
    automatic: Option<crate::audio::activity::Settings>,
) -> Result<()> {
    exact_id(id)?;
    ensure!(
        (5..=20).contains(&seconds),
        "PTP observation must be 5..=20 seconds"
    );
    ensure!(loaded.config.allows(id), "receiver is not approved");
    let store = store::Store::new(&loaded.path)?;
    let selected = store
        .selection()?
        .context("select this approved receiver first")?;
    ensure!(selected.id == id, "receiver is not selected");
    let (operation, credential) = wav_authentication(&store, &selected, transient)?;
    ensure!(
        selected.capabilities.features & ((1u64 << 9) | (1u64 << 19) | (1u64 << 41))
            == ((1u64 << 9) | (1u64 << 19) | (1u64 << 41)),
        "selected receiver must advertise audio, ALAC and PTP; no fallback"
    );
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "PTP operation requires interactive confirmation"
    );
    let snapshot = discovery::discover(loaded.config.discovery_timeout_secs)?;
    if transient {
        println!("Explicit OwnTone-reference transient SRP on the selected receiver: no persistent HAP credentials or verified persistent receiver identity; no credentials saved and no authentication fallback. Authenticated capabilities and PTP/type-96 negotiation must succeed before media transmission.");
    }
    if let Some(settings) = automatic {
        println!("Automatic selected receiver {id}: RMS threshold={} dBFS; start={} ms; silence={} ms; pre-roll capacity=10 seconds. Authorize repeated sessions until Ctrl+C; explicit volume changes may persist.", settings.threshold_dbfs, settings.start_ms, settings.stop_ms);
    } else if let Some((device, duration, volume)) = live {
        println!("Experimental live PTP capture: device={device:?}; seconds={duration:?} (None = Ctrl+C); volume={volume:?} dB (None preserves current volume). Explicit volume changes may persist. Stereo 44100 Hz S16; no activity detection.");
    } else if let Some((wav, volume)) = &playback {
        println!("Selected receiver: {id}; PTP type96, UDP, ALAC16 stereo, 44100 Hz, 352 samples/packet. WAV frames: {}; EOF: {}; volume: {volume:?} dB (None preserves current volume). Explicit volume changes may persist. Local transmission does not prove audible playback.", wav.frames.len(), wav.eof);
    } else {
        println!("PTP negotiation only: authenticated /info, initial SETUP, RECORD, SETPEERS, realtime type-96 UDP stream SETUP, {seconds}s bounded observation and TEARDOWN. Zero audio bytes; no volume changes. Reports operational readiness separately from unverified receiver synchronization.");
    }
    interactive::confirm_operation(
        loaded,
        None,
        &snapshot,
        if automatic.is_some() {
            "Authorize automatic sessions on this selected receiver until Ctrl+C? [y/N]: "
        } else if live.is_some() {
            "Start experimental selected receiver live PTP capture? [y/N]: "
        } else if playback.is_some() {
            "Start selected receiver PTP WAV playback? [y/N]: "
        } else {
            "Authorize selected receiver PTP negotiation only? [y/N]: "
        },
    )?;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::{Duration, Instant};
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))?;
    let capture = automatic
        .map(|settings| crate::audio::live::Capture::automatic(live.unwrap().0, settings))
        .transpose()?;
    let mut retry = automatic::Retry::new(Instant::now());
    let mut snapshot = snapshot;
    loop {
        if let Some(capture) = &capture {
            println!("Audio monitoring started/resumed; waiting for meaningful audio");
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(());
                }
                if retry.ready(Instant::now(), capture.activity()?.0) {
                    break;
                }
                std::thread::park_timeout(Duration::from_millis(20));
            }
            println!("Audio activity detected; AirPlay connection starting for explicitly selected receiver {id}");
        }
        let result = (|| {
            if capture.is_some() {
                snapshot = discovery::discover(loaded.config.discovery_timeout_secs)?;
            }
            let mut authenticated = session::Session::new();
            authenticated.cancel = cancel.clone();
            authenticated
                .authenticate_ptp_selected(
                    || match &credential {
                        Some(credential) => wire::Wire::connect_hap_operation(
                            &loaded.path,
                            &operation,
                            &snapshot,
                            credential,
                        ),
                        None => wire::Wire::connect(&loaded.path, id, &operation, &snapshot),
                    },
                    credential.as_ref(),
                )
                .context("selected receiver PTP authentication failed; no fallback")?;
            let mut session = authenticated.into_ptp_foundation()?;
            let result = (|| {
                session.setup_realtime_authenticated().context(
            "authenticated receiver capabilities or initial PTP SETUP rejected; advertised PTP support alone is insufficient; no fallback",
        )?;
                session.negotiate_realtime().context(
            "receiver rejected PTP realtime type-96 UDP negotiation; playback cannot proceed; no fallback",
        )?;
                if let Some(capture) = &capture {
                    session.play_automatic(capture.reader(), live.unwrap().2)?;
                } else if let Some((device, duration, volume)) = live {
                    session.play_live(device, duration, volume)?;
                } else if let Some((wav, volume)) = &playback {
                    session.play_wav(&wav.frames, *volume)?;
                } else {
                    session.observe(std::time::Duration::from_secs(seconds))?;
                }
                session.stop()
            })();
            println!("PTP operation: {}", session.diagnostic());
            result
        })();
        if capture.is_none() {
            return result;
        }
        println!("AirPlay session stopped; resources released");
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Err(error) = result {
            let delay = retry.failed(Instant::now());
            println!("AirPlay connection/playback failure: {error:#}; retry delay {delay}s (requires meaningful input)");
        } else {
            retry = automatic::Retry::new(Instant::now());
        }
    }
}
