//! Thin setup orchestration. Transport, authorization and persistence stay in
//! their existing modules; prompts never imply an authenticated receiver identity.
mod resources;
mod service;
mod ui;
use super::{
    compatibility::Method,
    runtime::{self, Authentication},
    store::Store,
};
use crate::{
    allowlist,
    audio::{activity, capture, live::Capture},
    cli::PlaybackArgs,
    config::{self, Loaded, Playback},
    discovery::{self, Receiver},
};
use anyhow::{ensure, Context, Result};
use std::{
    io::{self, IsTerminal, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use ui::{Cancelled, Ui};

pub(crate) fn run(loaded: Loaded) -> Result<()> {
    ui::require_terminal(io::stdin().is_terminal(), io::stdout().is_terminal())?;
    let cancel = runtime::signal_cancellation()?;
    let mut ui = Ui {
        read: || ui::terminal_line(&cancel),
        out: io::stdout(),
    };
    let mut journal = Vec::new();
    let result = (|| {
        ui.say("Welcome to PiAir setup.\nConfigure sound input and an AirPlay output.\nEnter numbered choices. q, EOF or Ctrl+C cancels.\nApproval, selection and pairing save separately.\nPlayback settings save only after your final review.\nNo packages or system settings will be installed.")?;
        ensure!(cfg!(target_os = "linux"), "PiAir requires Linux and ALSA");
        // Guard setup mutations and tests with the runtime's existing exclusion.
        let instance = runtime::InstanceLock::acquire()?;
        requirements(&loaded, &mut ui)?;
        let mut loaded = loaded;
        let mut baseline = allowlist::snapshot(&loaded.path)?;
        let mut draft = loaded.config.playback.clone();
        let receiver_name;
        loop {
            draft.capture_device = Some(select_input(&draft, &mut ui, &cancel)?);
            ui.say("1. Continue to AirPlay selection\n2. Back to audio input")?;
            if ui.choice("Choose:", 2, None)? == 1 {
                break;
            }
        }
        loop {
            let snapshot = discover_receivers(&loaded, &mut ui, &cancel)?;
            let id = match choose_receiver(&loaded, &snapshot, &mut ui) {
                Ok(id) => id,
                Err(e) if e.is::<Cancelled>() => return Err(e),
                Err(e) => {
                    ui.say(format!(
                        "Receiver validation failed: {e:#}\n1. Rescan\n2. Cancel"
                    ))?;
                    if ui.choice("Choose:", 2, None)? == 2 {
                        return Err(Cancelled.into());
                    }
                    continue;
                }
            };
            let Some(id) = id else { continue };
            ensure!(
                allowlist::snapshot(&loaded.path)? == baseline,
                "configuration changed during setup; rerun to review new settings"
            );
            approve(&mut loaded, &id, &mut ui, &mut journal)?;
            baseline = allowlist::snapshot(&loaded.path)?;
            match authentication(&loaded, &snapshot, &id, &mut ui, &mut journal, &cancel) {
                Ok(auth) => {
                    receiver_name = snapshot
                        .iter()
                        .find(|r| r.id == id)
                        .map(|r| clean(&r.name))
                        .unwrap_or_default();
                    draft.receiver_id = Some(id);
                    draft.authentication = Some(auth);
                    break;
                }
                Err(e) if e.is::<Cancelled>() => return Err(e),
                Err(e) => {
                    ui.say(format!("Authentication configuration failed: {e:#}\nNo fallback was attempted.\n1. Retry receiver/authentication selection\n2. Cancel"))?;
                    if ui.choice("Choose:", 2, None)? == 2 {
                        return Err(Cancelled.into());
                    }
                }
            }
        }
        // Approval changes above are intentional. Pin the remaining configuration
        // before allowing a long monitoring/test/review interval.
        configure_activity(&mut draft, &mut ui, &cancel)?;
        configure_volume(&mut draft, &mut ui)?;
        test_audio(&loaded, &draft, &mut ui, cancel.clone())?;
        let service_before = service::inspect(&loaded);
        loop {
            summary(&loaded.config.playback, &draft, &mut ui)?;
            ui.say(format!("Advertised receiver name: {receiver_name}"))?;
            ui.say(format!("Configuration: {}", loaded.path.display()))?;
            ui.say(format!(
                "Existing systemd service: {}",
                match &service_before {
                    Ok(s) => format!("{} ({})", s.unit, s.status),
                    Err(e) => format!("optional/unavailable: {e}"),
                }
            ))?;
            ui.say(
                "1. Save reviewed playback settings\n2. Back to detection and volume\n3. Cancel",
            )?;
            match ui.choice("Choose:", 3, None)? {
                1 => break,
                2 => {
                    configure_activity(&mut draft, &mut ui, &cancel)?;
                    configure_volume(&mut draft, &mut ui)?;
                }
                _ => return Err(Cancelled.into()),
            }
        }
        ensure!(!cancel.load(Ordering::Relaxed), Cancelled);
        config::save_playback_checked(
            &loaded,
            draft.receiver_id.clone(),
            playback_args(&draft),
            Some(&baseline),
        )?;
        journal.push(format!(
            "Playback settings saved to {}",
            loaded.path.display()
        ));
        ui.say("Playback settings saved. Restart PiAir explicitly to apply them.\nSecurity revocation checks remain active.")?;
        drop(instance);
        let loaded = config::load(Some(loaded.path.clone()))?;
        optional_service(&loaded, &mut ui, &mut journal)?;
        ui.say("Setup complete.\nTo run in this terminal: piair run --allow-unattended\nUse the same --config path if you supplied one.\nRe-run piair setup whenever you want to reconfigure.")?;
        Ok(())
    })();
    if result.is_err() {
        ui.say("Setup stopped. Completed explicit operations remain saved:")?;
        if journal.is_empty() {
            ui.say("None recorded by this setup invocation.")?;
        }
        for entry in journal {
            ui.say(entry)?;
        }
        ui.say("Any operation that reported an uncertain save/cleanup must be inspected.\nUnconfirmed playback settings were not saved.\nRe-run setup to resume; existing credentials are retained.")?;
    }
    match result {
        Err(e) if e.is::<Cancelled>() => Ok(()),
        other => other,
    }
}

fn requirements<R: FnMut() -> Result<String>, W: Write>(
    loaded: &Loaded,
    ui: &mut Ui<R, W>,
) -> Result<()> {
    ui.say("\n1/8 System requirements\nLinux: supported. ALSA is linked into this build.")?;
    if !std::path::Path::new("/dev/snd").is_dir() {
        ui.say("Warning: /dev/snd is missing. Connect your audio input, or check device access in your OS/container.")?;
    }
    let network = std::fs::read_dir("/sys/class/net")
        .map(|entries| entries.flatten().any(|e| e.file_name() != "lo"))
        .unwrap_or(false);
    if !network {
        ui.say("Warning: no non-loopback network interface found.")?;
    }
    ui.say("Connect PiAir and the receiver to the same local network.\nAllow mDNS multicast UDP 5353 and the documented AirPlay/PTP ports.\nA network interface does not prove receiver reachability.")?;
    ui.say(format!(
        "Configuration: {} ({})\nExisting playback: {}",
        loaded.path.display(),
        loaded.source,
        if loaded.config.playback.effective().is_ok() {
            "present; values will be reused for review"
        } else {
            "incomplete; setup will fill required values"
        }
    ))?;
    match service::inspect(loaded) {
        Ok(service) => ui.say(format!(
            "Installed service: {} ({})",
            service.unit, service.status
        ))?,
        Err(error) => {
            ui.say(format!("Optional systemd: {error}"))?;
            ui.say("Manual installation: docs/SYSTEMD.md. Installer planned for PR #74.")?;
        }
    }
    Ok(())
}
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn select_input<R: FnMut() -> Result<String>, W: Write>(
    draft: &Playback,
    ui: &mut Ui<R, W>,
    cancel: &AtomicBool,
) -> Result<String> {
    select_input_with(
        draft,
        ui,
        cancel,
        capture::discover_devices,
        Capture::validate_device,
    )
}
fn select_input_with<R: FnMut() -> Result<String>, W: Write>(
    draft: &Playback,
    ui: &mut Ui<R, W>,
    cancel: &AtomicBool,
    mut discover: impl FnMut() -> Result<Vec<(String, String)>>,
    mut validate: impl FnMut(&str) -> Result<()>,
) -> Result<String> {
    loop {
        ui.say(
            "\n2/8 Audio input\nChoose explicitly; device hints do not guarantee availability.",
        )?;
        let devices = match discover() {
            Ok(d) => d,
            Err(e) => {
                ui.say(format!("ALSA discovery failed: {e:#}"))?;
                Vec::new()
            }
        };
        if devices.is_empty() {
            ui.say("No capture devices found. Connect an input and retry,\nor enter a known ALSA identifier manually.")?;
        }
        for (i, (id, name)) in devices.iter().enumerate() {
            ui.say(format!("{}. {}\n   {}", i + 1, clean(name), clean(id)))?;
        }
        let manual = devices.len() + 1;
        ui.say(format!(
            "{manual}. Enter ALSA identifier manually\n{}. Rescan",
            manual + 1
        ))?;
        if let Some(id) = &draft.capture_device {
            ui.say(format!(
                "{}. Reuse current input: {}",
                manual + 2,
                clean(id)
            ))?;
        }
        let n = ui.choice(
            "Choose input (q cancels):",
            manual + 1 + usize::from(draft.capture_device.is_some()),
            None,
        )?;
        let device = if n == manual {
            ui.line("Enter exact ALSA identifier (for example plughw:CARD=<id>,DEV=0):")?
        } else if n == manual + 1 {
            continue;
        } else if n == manual + 2 {
            draft
                .capture_device
                .clone()
                .expect("menu requires current input")
        } else {
            devices[n - 1].0.clone()
        };
        ensure!(!cancel.load(Ordering::Relaxed), Cancelled);
        match validate(&device) {
            Ok(())=> {ui.say("Capture initialized successfully; validation handle closed.")?;return Ok(device);},
            Err(e)=>ui.say(format!("Cannot record from {}: {e:#}\nUnavailable: reconnect/check identifier.\nPermission denied: check your account's /dev/snd access.\nBusy: stop the other capture application yourself.\nUnsupported format: explicitly choose a plughw PCM for conversion.\nNo alternate input was substituted. Try again.",clean(&device)))?,
        }
    }
}
fn discover_receivers<R: FnMut() -> Result<String>, W: Write>(
    loaded: &Loaded,
    ui: &mut Ui<R, W>,
    cancel: &AtomicBool,
) -> Result<Vec<Receiver>> {
    discover_receivers_with(
        loaded.config.discovery_timeout_secs,
        ui,
        cancel,
        discovery::discover_cancellable,
    )
}
fn discover_receivers_with<R: FnMut() -> Result<String>, W: Write>(
    timeout: u64,
    ui: &mut Ui<R, W>,
    cancel: &AtomicBool,
    mut discover: impl FnMut(u64, &AtomicBool) -> Result<Vec<Receiver>>,
) -> Result<Vec<Receiver>> {
    loop {
        ui.say("\n3/8 AirPlay receiver\nDiscovering nearby advertisements...")?;
        match discover(timeout, cancel) {
            Ok(mut rows) => {
                ensure!(!cancel.load(Ordering::Relaxed), Cancelled);
                rows.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
                return Ok(rows);
            }
            Err(e) => {
                ensure!(!cancel.load(Ordering::Relaxed), Cancelled);
                ui.say(format!(
                    "Discovery failed: {e:#}\nCheck networking/multicast.\n1. Retry\n2. Cancel"
                ))?;
                if ui.choice("Choose:", 2, None)? == 2 {
                    return Err(Cancelled.into());
                }
            }
        }
    }
}
fn choose_receiver<R: FnMut() -> Result<String>, W: Write>(
    loaded: &Loaded,
    rows: &[Receiver],
    ui: &mut Ui<R, W>,
) -> Result<Option<String>> {
    let store = Store::new(&loaded.path)?;
    let selected = store.selection()?;
    if rows.is_empty() {
        ui.say("No receivers discovered. Check power and the local network.")?;
    }
    for (i, r) in rows.iter().enumerate() {
        let paired = super::exact_id(&r.id).is_ok() && store.credential(&r.id)?.is_some();
        ui.say(format!(
            "{}. {}\n   {}\n   Approved: {} Selected: {} Paired: {}",
            i + 1,
            clean(&r.name),
            r.id,
            loaded.config.allows(&r.id),
            selected.as_ref().is_some_and(|s| s.id == r.id),
            paired
        ))?;
    }
    ui.say(format!(
        "{}. Rescan\n{}. Cancel",
        rows.len() + 1,
        rows.len() + 2
    ))?;
    let n = ui.choice("Choose receiver:", rows.len() + 2, None)?;
    if n == rows.len() + 1 {
        return Ok(None);
    }
    if n == rows.len() + 2 {
        return Err(Cancelled.into());
    }
    let r = &rows[n - 1];
    super::exact_id(&r.id)?;
    ensure!(
        rows.iter().filter(|v| v.id == r.id).count() == 1,
        "receiver identity is ambiguous; rescan"
    );
    ui.say(format!(
        "Advertised name: {}\nCanonical receiver ID: {}",
        clean(&r.name),
        r.id
    ))?;
    for s in &r.services {
        ui.say(format!(
            "Endpoint: {} port {}\nAddresses: {:?}\nAdvertised capabilities: {:?}",
            clean(&s.service_type),
            s.port,
            s.addresses,
            s.capabilities
        ))?;
    }
    ui.say("mDNS advertisements are untrusted.\nThey do not prove the receiver's identity.")?;
    Ok(Some(r.id.clone()))
}
fn approve<R: FnMut() -> Result<String>, W: Write>(
    loaded: &mut Loaded,
    id: &str,
    ui: &mut Ui<R, W>,
    journal: &mut Vec<String>,
) -> Result<()> {
    if loaded.config.allows(id) {
        ui.say("Receiver already approved; approval unchanged.")?;
        return Ok(());
    }
    super::exact_id(id)?;
    let original = allowlist::snapshot(&loaded.path)?;
    if !ui.confirm(&format!(
        "Have you independently verified {id}\nusing its physical label or trusted local settings?"
    ))? {
        return Err(Cancelled.into());
    }
    if !ui.confirm(&format!(
        "Approve exactly {id} in\n{}?\nApproval allows later operations; it does not authenticate.",
        loaded.path.display()
    ))? {
        return Err(Cancelled.into());
    }
    let text = allowlist::render(original.as_ref().map_or("", |s| s.text.as_str()), id, true)?;
    allowlist::save(&loaded.path, original.as_ref(), &text)?;
    journal.push(format!("Approved {id}"));
    *loaded = config::load(Some(loaded.path.clone()))?;
    Ok(())
}

fn authentication<R: FnMut() -> Result<String>, W: Write>(
    loaded: &Loaded,
    rows: &[Receiver],
    id: &str,
    ui: &mut Ui<R, W>,
    journal: &mut Vec<String>,
    cancel: &Arc<AtomicBool>,
) -> Result<Authentication> {
    ui.say("\n4/8 Authentication\nChoose an explicit supported method. No automatic fallback.\nNames and models never choose a method.")?;
    let store = Store::new(&loaded.path)?;
    let previous = store.selection()?;
    let credential = store.credential(id)?;
    let receiver = rows
        .iter()
        .find(|r| r.id == id)
        .context("receiver missing")?;
    let mut options = Vec::new();
    if credential.is_some() {
        if let Ok(mut selected) = super::validate_selection(loaded, id, Method::StoredHap, rows) {
            // StoredHap remains operation-only. Persistent selection records a
            // persistent enrollment profile, never a new enrollment request.
            selected.method = previous
                .as_ref()
                .filter(|s| s.id == id && s.method != Method::Transient)
                .map_or(Method::SetupCode, |s| s.method);
            super::hap_credential(&store, &{
                let mut operation = selected.clone();
                operation.method = Method::StoredHap;
                operation
            })?;
            options.push((
                "Reuse validated stored HAP credentials",
                selected,
                Authentication::StoredHap,
                false,
            ));
        } else {
            ui.say("Stored credentials exist but their advertised binding or policy is invalid.\nThey will not be overwritten or deleted. Investigate before retrying.")?;
        }
    } else {
        for (method, label) in [
            (
                Method::PinDisplay,
                "PIN-display enrollment (requires actual supported PIN delivery)",
            ),
            (
                Method::SetupCode,
                "HAP setup-code enrollment (requires genuine independently obtained code)",
            ),
        ] {
            match super::Selection::from_receiver(receiver, method) {
                Ok(_) => options.push((
                    label,
                    super::validate_selection(loaded, id, method, rows)?,
                    Authentication::StoredHap,
                    true,
                )),
                Err(e) => ui.say(format!("{method:?} unavailable: {e}"))?,
            }
        }
    }
    match super::Selection::from_receiver(receiver, Method::Transient) {
        Ok(_) => options.push((
            "OwnTone-reference transient (session only)",
            super::validate_selection(loaded, id, Method::Transient, rows)?,
            Authentication::OwnToneTransient,
            false,
        )),
        Err(e) => ui.say(format!("Transient unavailable: {e}"))?,
    }
    ensure!(
        !options.is_empty(),
        "no supported authentication paths with current prerequisites; no fallback"
    );
    for (i, (label, _, _, _)) in options.iter().enumerate() {
        ui.say(format!("{}. {label}", i + 1))?;
    }
    let n = ui.choice("Choose authentication (q cancels):", options.len(), None)?;
    let (_, selected, auth, enroll) = &options[n - 1];
    if *enroll {
        let prompt = if selected.method == Method::PinDisplay {
            "Is an actual receiver-supported PIN display or independently verified PIN delivery available?"
        } else {
            "Do you have a genuine independently obtained HAP setup code?\nAn AirPlay password is not a HAP setup code."
        };
        ensure!(ui.confirm(prompt)?, Cancelled);
    }
    if *auth == Authentication::OwnToneTransient {
        ensure!(ui.confirm("Transient authentication does not establish verified persistent receiver identity.\nNo permanent credentials are enrolled.\nExplicitly use OwnTone-reference transient authentication?")?,Cancelled);
    }
    ensure!(ui.confirm(&format!("Save selection for exactly {id}\nwith method {:?} and the displayed endpoint?\nThis selection persists even if later setup is cancelled.",selected.method))?,Cancelled);
    let current = config::load(Some(loaded.path.clone()))?;
    let observed = super::validate_selection(&current, id, selected.method, rows)?;
    ensure!(
        selected.matches_observation(&observed),
        "receiver changed; retry explicitly"
    );
    store.select_if_unchanged(selected, previous.as_ref())?;
    journal.push(format!("Selected {id} ({:?})", selected.method));
    if *enroll {
        // Existing enrollment handles PIN prompting, cryptographic verification,
        // current selection/approval checks, no-clobber credential save and cleanup.
        ensure!(ui.confirm(&format!("Enroll and save verified credentials for exactly {id}?\nSelection is already saved; credentials save only after verification."))?, Cancelled);
        let snapshot =
            discovery::discover_cancellable(loaded.config.discovery_timeout_secs, cancel)?;
        ensure!(!cancel.load(Ordering::Relaxed), Cancelled);
        let mut wire = super::wire::Wire::connect(&loaded.path, id, selected, &snapshot)?;
        wire.diagnostic_cancel = Some(cancel.clone());
        let credential =
            super::pairing::setup(wire, id, selected.advertised_pi.as_deref(), || {
                ui.secret(if selected.method.pin_start() {
                    "Enter the actual PIN displayed/delivered by this receiver:"
                } else {
                    "Enter the genuine independently obtained HAP setup code:"
                })
            })?;
        ensure!(!cancel.load(Ordering::Relaxed), Cancelled);
        super::save_verified_enrollment(&loaded.path, selected, &credential)?;
        journal.push(format!("Enrolled verified HAP credentials for {id}"));
    }
    Ok(*auth)
}

fn detection(draft: &Playback) -> activity::Settings {
    activity::Settings {
        threshold_dbfs: draft.threshold_dbfs,
        start_ms: draft.start_duration_ms,
        stop_ms: draft.stop_duration_ms,
    }
}
fn measure<R: FnMut() -> Result<String>, W: Write>(
    draft: &Playback,
    ui: &mut Ui<R, W>,
    cancel: &AtomicBool,
) -> Result<Vec<f64>> {
    let capture = Capture::automatic(
        draft.capture_device.as_deref().context("missing input")?,
        detection(draft),
    )?;
    capture.wait_ready(cancel, || Ok(()))?;
    let end = Instant::now() + Duration::from_secs(5);
    let mut levels = Vec::new();
    while Instant::now() < end {
        ensure!(!cancel.load(Ordering::Relaxed), Cancelled);
        let level = capture.level()?;
        let active = capture.activity()?.0;
        levels.push(level);
        ui.say(format!(
            "Input {level:.1} dBFS | threshold {:.1}\nSustained audio detected: {active}",
            draft.threshold_dbfs
        ))?;
        std::thread::park_timeout(Duration::from_millis(200));
    }
    // Sole owner joins the worker and releases ALSA before the next prompt.
    drop(capture);
    Ok(levels)
}
pub(crate) fn suggested_threshold(noise: &[f64], signal: &[f64]) -> Option<f64> {
    // -infinity is a real RMS result for digital silence. NaN, +infinity
    // and out-of-range finite levels are not usable capture measurements.
    let valid = |level: &f64| {
        *level == f64::NEG_INFINITY || (level.is_finite() && (-120.0..=0.0).contains(level))
    };
    if noise.is_empty() || signal.is_empty() || !noise.iter().chain(signal).all(valid) {
        return None;
    }
    let mut noise = noise.to_vec();
    let mut signal = signal.to_vec();
    noise.sort_by(f64::total_cmp);
    signal.sort_by(f64::total_cmp);
    let quiet = noise[(noise.len() * 95).div_ceil(100) - 1];
    let music = signal[signal.len() / 4];
    // Require 6 dB on either side. Prefer 12 dB below quieter music rather
    // than following a very low (or digitally zero) background indefinitely.
    // The -60 dBFS suggestion floor avoids recommending extreme sensitivity;
    // weaker inputs need gain investigation or a deliberate manual setting.
    if !music.is_finite() || music - quiet < 12.0 {
        return None;
    }
    let threshold = (quiet + 6.0).max(music - 12.0).max(-60.0);
    (threshold <= music - 6.0).then_some(threshold)
}
fn configure_activity<R: FnMut() -> Result<String>, W: Write>(
    draft: &mut Playback,
    ui: &mut Ui<R, W>,
    cancel: &AtomicBool,
) -> Result<()> {
    loop {
        ui.say(format!("\n5/8 Automatic audio detection\nLouder levels are closer to 0 dBFS.\nThreshold: {} dBFS\nStart after: {} ms of sustained audio\nStop after: {} ms of silence\n1. Keep current/default settings\n2. Adjust manually\n3. Measure stopped source and music; suggest threshold",draft.threshold_dbfs,draft.start_duration_ms,draft.stop_duration_ms))?;
        match ui.choice("Choose:", 3, None)? {
            1 => return Ok(()),
            2 => {
                let mut candidate = draft.clone();
                candidate.threshold_dbfs = ui.number("Threshold in dBFS (-120 to 0):")?;
                candidate.start_duration_ms =
                    ui.number("Start after sustained audio, ms (20 to 9000):")?;
                candidate.stop_duration_ms = ui.number("Stop after silence, ms (20 to 120000):")?;
                match candidate.validate() {
                    Ok(()) => *draft = candidate,
                    Err(e) => {
                        ui.say(format!("Invalid settings: {e}; current settings retained."))?
                    }
                }
            }
            _ => {
                let result = (|| -> Result<Option<f64>> {
                    ensure!(ui.confirm("Stop the music source. Ready to measure background noise for 5 seconds?")?,Cancelled);
                    let noise = measure(draft, ui, cancel)?;
                    ensure!(
                        ui.confirm(
                            "Now play representative music. Ready to measure for 5 seconds?"
                        )?,
                        Cancelled
                    );
                    let signal = measure(draft, ui, cancel)?;
                    Ok(suggested_threshold(&noise, &signal))
                })();
                match result {
                    Ok(Some(value))=> {if ui.confirm(&format!("Measured suggestion: {value:.1} dBFS.\nThis is provisional; verify with your source. Apply to the draft?"))?{draft.threshold_dbfs=value;}},
                    Ok(None)=>ui.say("Measurements overlap or contain no useful signal.\nNo suggestion made; check gain/cables and retry.")?,
                    Err(e) if e.is::<Cancelled>()=>return Err(e),
                    Err(e)=>ui.say(format!("Measurement failed: {e:#}\nCapture released; retry or keep current settings."))?,
                }
            }
        }
    }
}
fn configure_volume<R: FnMut() -> Result<String>, W: Write>(
    draft: &mut Playback,
    ui: &mut Ui<R, W>,
) -> Result<()> {
    loop {
        ui.say(format!("Receiver volume: {:?} dB (None preserves current volume)\n1. Keep current configuration\n2. Preserve receiver volume\n3. Set an explicit volume",draft.volume_db))?;
        match ui.choice("Choose volume:", 3, Some(1))? {
            1 => return Ok(()),
            2 => {
                draft.volume_db = None;
                return Ok(());
            }
            _ => {
                let value: i32 =
                    ui.number("Volume dB (-60 to -10). Changes may persist on receiver:")?;
                if !(-60..=-10).contains(&value) {
                    ui.say("Volume out of range.")?;
                    continue;
                }
                if ui.confirm(&format!("Save an explicit {value} dB receiver volume?\nThe change may persist on the receiver after playback or failure."))?{draft.volume_db=Some(value);return Ok(())}
            }
        }
    }
}
fn test_audio<R: FnMut() -> Result<String>, W: Write>(
    loaded: &Loaded,
    draft: &Playback,
    ui: &mut Ui<R, W>,
    cancel: Arc<AtomicBool>,
) -> Result<()> {
    ui.say("\n6/8 AirPlay audio playback test\nTests the AirPlay output path; ALSA input was checked separately.\n1. Play test recording (49 seconds)\n2. Skip test [default]")?;
    if ui.choice("Choose:", 2, Some(2))? == 2 {
        return Ok(());
    }
    let Some(path) = resources::locate() else {
        ui.say("Test recording or LICENSE.md is missing. Continuing without playback.\nInstall both in /usr/local/share/piair/audio (future installer PR #74),\nor set PIAIR_AUDIO_DATA_DIR to their directory.\nDevelopment: tests/assets/audio. Nothing is downloaded.")?;
        return Ok(());
    };
    ui.say(format!("Pachelbel's Canon in D, arrangement/performance: Lee Galloway.\nRecording: CC BY-SA 3.0, separately from PiAir's Apache-2.0 code.\nAttribution: {}\nhttps://creativecommons.org/licenses/by-sa/3.0/\nReceiver volume is preserved during this test. Ctrl+C stops playback.",path.with_file_name("LICENSE.md").display()))?;
    let result = super::wav::Playback::open(&path, 49).and_then(|wav| {
        super::ptp_operation_cancellable(
            loaded,
            draft.receiver_id.as_deref().context("missing receiver")?,
            draft.authentication == Some(Authentication::OwnToneTransient),
            10,
            Some((wav, None)),
            None,
            Some(cancel.clone()),
        )
    });
    playback_feedback(result, ui, &cancel)
}

fn playback_troubleshooting<R: FnMut() -> Result<String>, W: Write>(
    ui: &mut Ui<R, W>,
) -> Result<()> {
    ui.say("Audio is not verified. Check receiver volume/mute and speakers.\nCheck the local network and PTP UDP 319/320 permissions.\nSee docs/PTP_PORT_PERMISSIONS.md and docs/WAV_PLAYBACK_DIAGNOSTIC.md.")
}

fn playback_feedback<R: FnMut() -> Result<String>, W: Write>(
    result: Result<()>,
    ui: &mut Ui<R, W>,
    cancel: &AtomicBool,
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return match result {
            Err(e) if e.is::<super::lifecycle::CleanupFailure>() => Err(e),
            _ => Err(Cancelled.into()),
        };
    }
    if let Err(e) = result {
        ui.say(format!("Playback test failed: {e:#}"))?;
        playback_troubleshooting(ui)?;
        ui.say("Continuing setup without a successful playback test.")?;
        return Ok(());
    }
    ui.say("Did you hear the audio?\n1. Yes, audio works\n2. No, show troubleshooting guidance\n3. Skip confirmation")?;
    match ui.choice("Choose:", 3, None)? {
        1 => ui.say("You confirmed audible output from this test.")?,
        2 => playback_troubleshooting(ui)?,
        _ => ui.say("Audible playback remains unverified.")?,
    }
    Ok(())
}
fn playback_args(p: &Playback) -> PlaybackArgs {
    PlaybackArgs {
        capture_device: p.capture_device.clone(),
        authentication: p.authentication,
        threshold_dbfs: Some(p.threshold_dbfs),
        start_duration_ms: Some(p.start_duration_ms),
        stop_duration_ms: Some(p.stop_duration_ms),
        volume_db: p.volume_db,
        preserve_volume: p.volume_db.is_none(),
    }
}
fn authentication_label(authentication: Option<Authentication>) -> &'static str {
    match authentication {
        Some(Authentication::StoredHap) => "Stored HAP credentials",
        Some(Authentication::OwnToneTransient) => "OwnTone-reference transient (session only)",
        None => "Not configured",
    }
}
fn volume_label(volume: Option<i32>) -> String {
    volume.map_or_else(
        || "Preserve current receiver volume".to_owned(),
        |v| format!("{v} dB (may persist)"),
    )
}
fn summary<R: FnMut() -> Result<String>, W: Write>(
    old: &Playback,
    new: &Playback,
    ui: &mut Ui<R, W>,
) -> Result<()> {
    ui.say("\n7/8 Review playback settings (* = changed)")?;
    for (label, before, after) in [
        (
            "Audio input",
            old.capture_device
                .as_deref()
                .unwrap_or("Not configured")
                .to_owned(),
            new.capture_device
                .as_deref()
                .unwrap_or("Not configured")
                .to_owned(),
        ),
        (
            "AirPlay receiver",
            old.receiver_id
                .as_deref()
                .unwrap_or("Not configured")
                .to_owned(),
            new.receiver_id
                .as_deref()
                .unwrap_or("Not configured")
                .to_owned(),
        ),
        (
            "Authentication profile",
            authentication_label(old.authentication).to_owned(),
            authentication_label(new.authentication).to_owned(),
        ),
        (
            "Detection threshold (dBFS)",
            old.threshold_dbfs.to_string(),
            new.threshold_dbfs.to_string(),
        ),
        (
            "Start duration (ms)",
            old.start_duration_ms.to_string(),
            new.start_duration_ms.to_string(),
        ),
        (
            "Stop duration (ms)",
            old.stop_duration_ms.to_string(),
            new.stop_duration_ms.to_string(),
        ),
        (
            "Receiver volume",
            volume_label(old.volume_db),
            volume_label(new.volume_db),
        ),
    ] {
        ui.say(format!(
            "{} {label}: {}{}",
            if before == after { " " } else { "*" },
            clean(&after),
            if before == after {
                String::new()
            } else {
                format!("\n  Previously: {}", clean(&before))
            }
        ))?;
    }
    Ok(())
}
fn optional_service<R: FnMut() -> Result<String>, W: Write>(
    loaded: &Loaded,
    ui: &mut Ui<R, W>,
    journal: &mut Vec<String>,
) -> Result<()> {
    optional_service_with(loaded, ui, journal, service::inspect, service::activate)
}
fn optional_service_with<R: FnMut() -> Result<String>, W: Write>(
    loaded: &Loaded,
    ui: &mut Ui<R, W>,
    journal: &mut Vec<String>,
    inspect: impl FnOnce(&Loaded) -> Result<service::Service>,
    activate: impl FnOnce(&Loaded, &service::Service) -> Result<()>,
) -> Result<()> {
    ui.say("\n8/8 Optional background service")?;
    let service = match inspect(loaded) {
        Ok(s) => s,
        Err(e) => {
            ui.say(format!("Automatic activation unavailable: {e}\nSetup can finish successfully without a service.\nManual procedure: docs/SYSTEMD.md; installer planned for PR #74."))?;
            return Ok(());
        }
    };
    ui.say("Start PiAir automatically when Raspberry Pi boots?\n1. Yes\n2. No [default]")?;
    if ui.choice("Choose:", 2, Some(2))? == 2 {
        return Ok(());
    }
    if !ui.confirm(&format!("Enable and start {} now?\nAccount: {} (non-root)\nConfiguration: {}\nThis uses systemctl enable --now with normal system authorization.\nPlayback starts when sustained audio is detected.\nNo other user's service will be stopped.",service.unit,service.user,loaded.path.display()))?{return Ok(())}
    match activate(loaded, &service) {
        Ok(()) => {
            journal.push(format!("Enabled and started {}", service.unit));
            ui.say("Service enabled and started.")?;
        }
        Err(error) => {
            ui.say(format!("Service activation failed: {error:#}"))?;
            ui.say("Playback configuration is saved. Inspect systemctl status and enablement.")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
