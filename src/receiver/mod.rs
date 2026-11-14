//! Explicit receiver operations. Discovery/audio never depend on this module.
mod automatic;
pub(crate) mod compatibility;
mod crypto;
mod diagnostics;
mod hex_u64;
pub(crate) mod interactive;
pub(crate) mod lifecycle;
mod pairing;
mod playback;
mod playback_error;
mod profile;
mod ptp;
mod realtime;
pub(crate) mod runtime;
mod session;
pub(crate) mod setup;
mod srp;
mod store;
#[cfg(test)]
mod tests;
mod tlv;
mod wav;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
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
    /// All addresses are approved endpoints and participate in cross-identity
    /// conflict checks. Even an IPv6-only addition requires explicit reselection:
    /// transient authentication cannot verify a persistent receiver identity.
    /// See docs/SECURITY.md before relaxing this boundary.
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
            display_name: Some(r.name.clone()),
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
    validate_receiver_observation(loaded, id, method, snapshot)
}
// Read-only prerequisites, including for an unapproved candidate. This grants
// no authorization; persistence and connection still require validate_selection.
fn validate_receiver_observation(
    loaded: &Loaded,
    id: &str,
    method: Method,
    snapshot: &[Receiver],
) -> Result<Selection> {
    exact_id(id)?;
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
/// Retry only absence, never a rejected observation. Each attempt is a new,
/// bounded mDNS snapshot; no endpoint from an earlier attempt is reused.
fn playback_discovery(
    loaded: &Loaded,
    selected: &Selection,
    cancel: &std::sync::atomic::AtomicBool,
    discover: impl FnMut(u64, &std::sync::atomic::AtomicBool) -> Result<Vec<Receiver>>,
) -> Result<Vec<Receiver>> {
    playback_discovery_inner(loaded, selected, cancel, discover)
        .context(playback_error::Stage::Discovery)
}
fn playback_discovery_inner(
    loaded: &Loaded,
    selected: &Selection,
    cancel: &std::sync::atomic::AtomicBool,
    mut discover: impl FnMut(u64, &std::sync::atomic::AtomicBool) -> Result<Vec<Receiver>>,
) -> Result<Vec<Receiver>> {
    use std::sync::atomic::Ordering;
    for attempt in 0..3 {
        ensure!(
            !cancel.load(Ordering::Relaxed),
            "receiver discovery cancelled; authentication not started"
        );
        let snapshot = discover(loaded.config.discovery_timeout_secs, cancel)
            .context("receiver discovery failed; authentication not started")?;
        ensure!(
            !cancel.load(Ordering::Relaxed),
            "receiver discovery cancelled; authentication not started"
        );
        let count = snapshot.iter().filter(|r| r.id == selected.id).count();
        // A conflicting identity must not be concealed by a retry, even if the
        // selected ID is absent from this snapshot.
        ensure!(
            !snapshot
                .iter()
                .filter(|r| r.id != selected.id)
                .flat_map(|r| &r.services)
                .any(|s| s.addresses.iter().any(|ip| selected.addresses.contains(ip))),
            "receiver discovery has conflicting identities; authentication not started"
        );
        if count == 0 {
            tracing::debug!(
                attempt = attempt + 1,
                "selected receiver missing from fresh discovery snapshot"
            );
            continue;
        }
        ensure!(
            count == 1,
            "receiver discovery identity is ambiguous; authentication not started"
        );
        let observed = validate_selection(loaded, &selected.id, selected.method, &snapshot)
            .context("receiver discovery validation failed; authentication not started")?;
        ensure!(selected.matches_observation(&observed),
            "receiver identity/capability/endpoint changed; review and select again; authentication not started");
        return Ok(snapshot);
    }
    anyhow::bail!("selected speaker was not found after 3 discovery attempts; check its power and network, then retry; authentication not started")
}
pub fn unpair(loaded: &Loaded, id: &str) -> Result<()> {
    exact_id(id)?;
    store::Store::new(&loaded.path)?.remove(id)?;
    println!("Local credentials removed for {id}. Receiver-side pairing and allowlist unchanged; no connection made.");
    Ok(())
}
pub fn pair(loaded: &Loaded, id: &str) -> Result<()> {
    pair_with_selection(loaded, id, None)
}
fn pair_with_selection(
    loaded: &Loaded,
    id: &str,
    proposed: Option<interactive::ProposedSelection>,
) -> Result<()> {
    exact_id(id)?;
    ensure!(loaded.config.allows(id), "receiver is not approved");
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "pair requires an interactive terminal"
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
        selected.method != Method::Transient,
        "transient selection does not enroll credentials; use setup for session-only playback"
    );
    ensure!(
        store.credential(id)?.is_none(),
        "pair requires no existing credentials"
    );
    println!("Persistent enrollment requires receiver-supported PIN delivery or a genuine independently obtained HAP setup code. An AirPlay password is not a HAP setup code. Do not invent codes or guess methods.");
    let snapshot = discovery::discover(loaded.config.discovery_timeout_secs)?;
    interactive::describe(&snapshot, id)?;
    println!("Operation: Pair and save verified credentials (no audio).");
    interactive::confirm_operation(loaded, proposed.as_ref(), &snapshot, "Continue? [y/N]: ")?;
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
    println!("Paired {id}; verified credentials saved. Connection closed. Start playback through setup or the daily menu.");
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
            "transient authentication requires explicit transient selection; no fallback"
        );
        selected.capabilities.decide(Method::Transient)?;
        return Ok((operation, None));
    }
    ensure!(
        selected.method != Method::Transient,
        "Playback requires a persistent receiver selection and existing HAP credentials, or an explicitly configured transient method; no fallback"
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

fn recording_playback(
    loaded: &Loaded,
    id: &str,
    transient: bool,
    wav: wav::Playback,
    volume: Option<i32>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    exact_id(id)?;
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
    let snapshot =
        playback_discovery(loaded, &operation, &cancel, discovery::discover_cancellable)?;
    let mut authenticated = session::Session::new();
    authenticated.cancel = cancel;
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
        .context("selected receiver PTP authentication failed; no fallback")
        .context(playback_error::Stage::Connection)?;
    let mut session = authenticated
        .into_ptp_foundation()
        .context(playback_error::Stage::Connection)?;
    let result = (|| {
        session.setup_realtime_authenticated().context(
            "authenticated receiver capabilities or initial PTP SETUP rejected; advertised PTP support alone is insufficient; no fallback",
        )?;
        session
            .negotiate_realtime()
            .context("receiver rejected PTP realtime type-96 UDP negotiation; playback cannot proceed; no fallback")?;
        session.play_wav(&wav.frames, volume)?;
        session.stop()
    })();
    tracing::debug!(diagnostic = %session.diagnostic(), "playback test completed");
    result.context(playback_error::Stage::Playback)
}
