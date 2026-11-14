//! Thin setup orchestration. Transport, authorization and persistence stay in
//! their existing modules; prompts never imply an authenticated receiver identity.
mod daily;
mod inputs;
use inputs::normal_inputs;
mod resources;
pub(crate) use daily::{daily, daily_control, daily_logs, daily_status};
mod recording;
mod service;
pub(crate) use recording::worker as service_recording;
pub(crate) fn recording_cleanup_failed(error: &anyhow::Error) -> bool {
    error.is::<super::lifecycle::CleanupFailure>()
}
pub(crate) use service::require_installed_executable;
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

/// Presentation context only: the original CleanupFailure remains in the chain.
#[derive(Debug)]
pub(crate) struct FatalPlayback;
impl std::fmt::Display for FatalPlayback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PiAir couldn't safely finish the audio test. Setup stopped. Check the speaker before trying again. Run piair -v setup for details.")
    }
}
impl std::error::Error for FatalPlayback {}

pub(crate) fn command_error_message(error: &anyhow::Error, detailed: bool) -> String {
    if !detailed && error.is::<FatalPlayback>() {
        FatalPlayback.to_string()
    } else {
        format!("{error:#}")
    }
}

fn setup_problem(error: &anyhow::Error, guidance: &str, detailed: bool) -> String {
    tracing::debug!(error = %format!("{error:#}"), "setup operation failed");
    if detailed || error.is::<super::lifecycle::CleanupFailure>() {
        format!("{guidance}\nDetails: {error:#}")
    } else {
        format!("{guidance}\nRun piair -v setup for details.")
    }
}

pub(crate) fn run(loaded: Loaded) -> Result<()> {
    ui::require_terminal(io::stdin().is_terminal(), io::stdout().is_terminal())?;
    let cancel = runtime::signal_cancellation()?;
    let mut ui = Ui {
        read: || ui::terminal_line(&cancel),
        out: io::stdout(),
    };
    let mut journal = Vec::new();
    let result = (|| {
        ui.say("Welcome to PiAir setup.\nChoose your audio input and speaker.\nEnter a number to choose, or q to cancel.")?;
        ensure!(cfg!(target_os = "linux"), "PiAir requires Linux and ALSA");
        // Guard setup mutations and tests with the runtime's existing exclusion.
        let interrupted_for_setup = if let Ok(current) = service::inspect_current(&loaded) {
            if current.active == "active" && current.pid != 0 {
                let _installed = service::require_installed_executable()?;
                if !ui
                    .confirm("Stop PiAir to change settings? You can start it again afterward.")?
                {
                    ui.say("Setup cancelled; automatic playback is unchanged.")?;
                    return Ok(());
                }
                service::control(&loaded, "stop")?;
                journal.push("Automatic playback explicitly stopped for setup. It will not restart without approval; use piair start when ready.".into());
                true
            } else {
                false
            }
        } else {
            false
        };
        let instance = runtime::InstanceLock::acquire()?;
        requirements(&mut ui)?;
        let mut loaded = loaded;
        let mut baseline = allowlist::snapshot(&loaded.path)?;
        let mut draft = loaded.config.playback.clone();
        let receiver_name;
        draft.capture_device = Some(select_input(&draft, &mut ui, &cancel)?);
        loop {
            let snapshot = discover_receivers(&loaded, &mut ui, &cancel)?;
            let id = match choose_receiver(&loaded, &snapshot, &mut ui) {
                Ok(id) => id,
                Err(e) if e.is::<Cancelled>() => return Err(e),
                Err(e) => {
                    ui.say(setup_problem(&e, "Could not validate the speaker. Check that it is available and scan again.\n1. Rescan\n2. Cancel", tracing::enabled!(tracing::Level::DEBUG)))?;
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
            let name = &snapshot
                .iter()
                .find(|r| r.id == id)
                .context("receiver missing")?
                .name;
            approve_named(&mut loaded, &id, name, &mut ui, &mut journal)?;
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
                    ui.say(setup_problem(&e, "Could not configure the speaker connection. Check the connection method and saved pairing.\n1. Choose speaker and connection again\n2. Cancel", tracing::enabled!(tracing::Level::DEBUG)))?;
                    if ui.choice("Choose:", 2, None)? == 2 {
                        return Err(Cancelled.into());
                    }
                }
            }
        }
        // Approval changes above are intentional. Pin the remaining configuration
        // before allowing a long monitoring/test/review interval.
        advanced_settings(&mut draft, &mut ui, &cancel)?;

        loop {
            ui.say(format!("\n5. Review settings\nAudio input: {}\nSpeaker: {receiver_name}\nConnection: {}\nAudio detection: automatic\nVolume: {}", clean(draft.capture_device.as_deref().unwrap_or("Not configured")), authentication_label(draft.authentication), volume_label(draft.volume_db)))?;
            ui.say("1. Save settings\n2. Advanced settings\n3. Cancel")?;
            match ui.choice("Choose:", 3, None)? {
                1 => break,
                2 => {
                    summary(&loaded.config.playback, &draft, &mut ui)?;
                    configure_activity(&mut draft, &mut ui, &cancel)?;
                    configure_volume(&mut draft, &mut ui)?;
                }
                _ => return Err(Cancelled.into()),
            }
        }
        ensure!(!cancel.load(Ordering::Relaxed), Cancelled);
        let mut reviewed =
            config::Config::parse(baseline.as_ref().map_or("", |s| s.text.as_str()))?;
        reviewed.playback = draft.clone();
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
        ui.say("Settings saved.")?;
        drop(instance);
        let saved = recording::fingerprint(&loaded.path)?;
        let loaded = config::load(Some(loaded.path.clone()))?;
        if toml::to_string(&loaded.config)? != toml::to_string(&reviewed)? {
            ui.say("Settings were saved, but configuration changed afterward. Startup and testing skipped. Review the current settings with piair setup.")?;
            return Ok(());
        }
        recording::check_saved(&loaded, &saved)?;
        optional_service(&loaded, &mut ui, &mut journal)?;
        let restore_after_setup = interrupted_for_setup
            && service::inspect_current(&loaded)
                .is_ok_and(|s| matches!(s.active.as_str(), "inactive" | "failed") && s.pid == 0);
        let test = recording::offer(&loaded, &saved, &mut ui, &cancel)?;
        if restore_after_setup
            && test.safe_to_restore
            && !cancel.load(Ordering::Relaxed)
            && service::inspect_current(&loaded)
                .is_ok_and(|s| matches!(s.active.as_str(), "inactive" | "failed") && s.pid == 0)
            && ui.confirm("Start PiAir again?")?
        {
            if let Err(e) = service::control(&loaded, "start") {
                ui.say(format!("Could not restore playback: {e:#}. Inspect piair status, then explicitly run piair start."))?;
            }
        }
        ui.say(completion_text(
            &loaded,
            service::observe(&loaded),
            test.outcome,
        )?)?;
        Ok(())
    })();
    if result.is_err() {
        ui.say("Setup stopped. Saved changes:")?;
        if journal.is_empty() {
            ui.say("None.")?;
        }
        for entry in journal {
            ui.say(entry)?;
        }
        ui.say("If a save or cleanup failed, check the reported error before continuing.\nRun piair setup to resume. Existing pairing is kept.")?;
    }
    match result {
        Err(e) if e.is::<Cancelled>() => Ok(()),
        other => other,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TestOutcome {
    Skipped,
    Passed,
    Failed,
}
impl TestOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Skipped => "Skipped",
            Self::Passed => "Passed",
            Self::Failed => "Failed",
        }
    }
}
fn completion_text(
    loaded: &Loaded,
    observed: Result<service::Service>,
    test: TestOutcome,
) -> Result<String> {
    let selected = Store::new(&loaded.path)?.selection()?;
    let speaker = selected
        .as_ref()
        .filter(|s| Some(&s.id) == loaded.config.playback.receiver_id.as_ref())
        .map(|s| clean(s.display_name.as_deref().unwrap_or(&s.id)))
        .unwrap_or_else(|| "Not selected".into());
    let startup = observed
        .map(|s| daily::boot_label(&s.boot))
        .unwrap_or_else(|_| "Unavailable".into());
    let troubleshooting = if test == TestOutcome::Failed {
        "\nCheck the speaker volume and connection. Run piair logs for details."
    } else {
        ""
    };
    Ok(format!("Setup complete.\n\nSpeaker: {speaker}\nAudio input: {}\nConnection: {}\nAutomatic startup: {startup}\nAudio test: {}{troubleshooting}\n\nNext: Play audio through your connected input.\nUse piair status to check PiAir, or piair start when ready.",
        clean(loaded.config.playback.capture_device.as_deref().unwrap_or("Not configured")),
        authentication_label(loaded.config.playback.authentication), test.label()))
}

fn requirements<R: FnMut() -> Result<String>, W: Write>(ui: &mut Ui<R, W>) -> Result<()> {
    ui.say("Connect your audio input and put your speaker on the same network as this Pi.")?;
    if !std::path::Path::new("/dev/snd").is_dir() {
        ui.say("Audio input unavailable. Connect your input and check sound-device access.")?;
    }
    let network = std::fs::read_dir("/sys/class/net")
        .map(|entries| entries.flatten().any(|e| e.file_name() != "lo"))
        .unwrap_or(false);
    if !network {
        ui.say("No network connection found. Connect this Pi to your speaker’s network.")?;
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
        ui.say("\n1. Choose audio input")?;
        let devices = match discover() {
            Ok(d) => d,
            Err(e) => {
                ui.say(setup_problem(
                    &e,
                    "Could not find audio inputs. Check the USB connection and try again.",
                    tracing::enabled!(tracing::Level::DEBUG),
                ))?;
                Vec::new()
            }
        };
        let primary = normal_inputs(devices.clone());
        if primary.is_empty() {
            ui.say("No audio inputs found. Connect your input and scan again,\nor use Advanced options to enter an ALSA identifier.")?;
        }
        for (i, (id, name)) in primary.iter().enumerate() {
            ui.say(format!(
                "{}. {}",
                i + 1,
                clean(if name.is_empty() { id } else { name })
            ))?;
        }
        let advanced = primary.len() + 1;
        ui.say(format!(
            "{advanced}. Other inputs / Advanced options\n{}. Rescan",
            advanced + 1
        ))?;
        let n = ui.choice("Choose input (q cancels):", advanced + 1, None)?;
        let device = if n == advanced {
            match advanced_input(&devices, draft, ui)? {
                Some(id) => id,
                None => continue,
            }
        } else if n == advanced + 1 {
            continue;
        } else {
            primary[n - 1].0.clone()
        };
        ensure!(!cancel.load(Ordering::Relaxed), Cancelled);
        match validate(&device) {
            Ok(())=> {ui.say("Audio input is ready.")?;return Ok(device);},
            Err(e)=>ui.say(setup_problem(&e, "Audio input unavailable. Check the USB connection, input permissions and other audio applications.\nFor format errors, choose the input’s plughw option under Advanced options.", tracing::enabled!(tracing::Level::DEBUG)))?,
        }
    }
}
fn advanced_input<R: FnMut() -> Result<String>, W: Write>(
    devices: &[(String, String)],
    draft: &Playback,
    ui: &mut Ui<R, W>,
) -> Result<Option<String>> {
    ui.say("Advanced audio inputs (exact ALSA identifiers):")?;
    for (i, (id, name)) in devices.iter().enumerate() {
        ui.say(format!("{}. {}\n   {}", i + 1, clean(name), clean(id)))?;
    }
    let manual = devices.len() + 1;
    ui.say(format!(
        "{manual}. Enter ALSA identifier manually\n{}. Back",
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
        "Choose:",
        manual + 1 + usize::from(draft.capture_device.is_some()),
        None,
    )?;
    Ok(if n == manual {
        Some(ui.line("Enter exact ALSA identifier:")?)
    } else if n == manual + 1 {
        None
    } else if n == manual + 2 {
        draft.capture_device.clone()
    } else {
        Some(devices[n - 1].0.clone())
    })
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
        ui.say("\n2. Choose AirPlay speaker\nLooking for speakers...")?;
        match discover(timeout, cancel) {
            Ok(mut rows) => {
                ensure!(!cancel.load(Ordering::Relaxed), Cancelled);
                rows.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
                return Ok(rows);
            }
            Err(e) => {
                ensure!(!cancel.load(Ordering::Relaxed), Cancelled);
                ui.say(format!(
                    "Could not find speakers: {e}\nCheck the network connection and try again.\n1. Retry\n2. Cancel"
                ))?;
                if ui.choice("Choose:", 2, None)? == 2 {
                    return Err(Cancelled.into());
                }
            }
        }
    }
}
fn choose_receiver<R: FnMut() -> Result<String>, W: Write>(
    _loaded: &Loaded,
    rows: &[Receiver],
    ui: &mut Ui<R, W>,
) -> Result<Option<String>> {
    if rows.is_empty() {
        ui.say(
            "No speakers found. Check that your speaker is on and connected to the same network.",
        )?;
    }
    for (i, r) in rows.iter().enumerate() {
        ui.say(format!("{}. {}", i + 1, clean(&r.name)))?;
    }
    ui.say(format!(
        "{}. Scan again\n{}. Cancel",
        rows.len() + 1,
        rows.len() + 2
    ))?;
    let n = ui.choice("Choose a speaker:", rows.len() + 2, None)?;
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
        "Speaker identity is ambiguous. Scan again."
    );
    Ok(Some(r.id.clone()))
}
#[cfg(test)]
fn approve<R: FnMut() -> Result<String>, W: Write>(
    loaded: &mut Loaded,
    id: &str,
    ui: &mut Ui<R, W>,
    journal: &mut Vec<String>,
) -> Result<()> {
    approve_named(loaded, id, "Room", ui, journal)
}
fn approve_named<R: FnMut() -> Result<String>, W: Write>(
    loaded: &mut Loaded,
    id: &str,
    name: &str,
    ui: &mut Ui<R, W>,
    journal: &mut Vec<String>,
) -> Result<()> {
    super::exact_id(id)?;
    if loaded.config.allows(id) {
        return Ok(());
    }
    let original = allowlist::snapshot(&loaded.path)?;
    if !ui.confirm(&format!("Use \"{}\" as your speaker?", clean(name)))? {
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
    ui.say("\n3. Choose connection method")?;
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
                "Use saved pairing",
                selected,
                Authentication::StoredHap,
                false,
            ));
        } else {
            ui.say("Stored credentials exist but their advertised binding or policy is invalid.\nThey will not be overwritten or deleted. Investigate before retrying.")?;
        }
    } else {
        for (method, label) in [
            (Method::PinDisplay, "Pair using a PIN from the speaker"),
            (Method::SetupCode, "Pair using a HAP setup code"),
        ] {
            match super::Selection::from_receiver(receiver, method) {
                Ok(_) => options.push((
                    label,
                    super::validate_selection(loaded, id, method, rows)?,
                    Authentication::StoredHap,
                    true,
                )),
                Err(e) => tracing::debug!(error = %e, ?method, "connection method unavailable"),
            }
        }
    }
    match super::Selection::from_receiver(receiver, Method::Transient) {
        Ok(_) => options.push((
            "Connect without pairing",
            super::validate_selection(loaded, id, Method::Transient, rows)?,
            Authentication::OwnToneTransient,
            false,
        )),
        Err(e) => tracing::debug!(error = %e, "session connection unavailable"),
    }
    ensure!(
        !options.is_empty(),
        "No supported connection method is available for this speaker. Run piair -v setup for details."
    );
    for (i, (label, _, _, _)) in options.iter().enumerate() {
        ui.say(format!("{}. {label}", i + 1))?;
    }
    let n = ui.choice("Choose connection:", options.len(), None)?;
    let (_, selected, auth, enroll) = &options[n - 1];
    if *enroll {
        let prompt = if selected.method == Method::PinDisplay {
            "Can you get the PIN from your speaker?"
        } else {
            "Do you have the speaker’s HAP setup code?\nUse its setup code, not an AirPlay password."
        };
        ensure!(ui.confirm(prompt)?, Cancelled);
    }
    if *auth == Authentication::OwnToneTransient {
        ensure!(
            ui.confirm("Connect without pairing for this session?")?,
            Cancelled
        );
    }
    ensure!(ui.confirm("Save this speaker and connection method?\nThis is saved even if you cancel setup later.")?,Cancelled);
    let current = config::load(Some(loaded.path.clone()))?;
    let observed = super::validate_selection(&current, id, selected.method, rows)?;
    ensure!(
        selected.matches_observation(&observed),
        "The speaker changed during setup. Select it again."
    );
    store.select_if_unchanged(selected, previous.as_ref())?;
    journal.push(format!("Selected {id} ({:?})", selected.method));
    if *enroll {
        // Existing enrollment handles PIN prompting, cryptographic verification,
        // current selection/approval checks, no-clobber credential save and cleanup.
        ensure!(
            ui.confirm("Pair with this speaker and save the pairing?")?,
            Cancelled
        );
        let snapshot =
            discovery::discover_cancellable(loaded.config.discovery_timeout_secs, cancel)?;
        ensure!(!cancel.load(Ordering::Relaxed), Cancelled);
        let mut wire = super::wire::Wire::connect(&loaded.path, id, selected, &snapshot)?;
        wire.diagnostic_cancel = Some(cancel.clone());
        let credential =
            super::pairing::setup(wire, id, selected.advertised_pi.as_deref(), || {
                ui.secret(if selected.method.pin_start() {
                    "Enter the PIN from your speaker:"
                } else {
                    "Enter the speaker’s HAP setup code:"
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
fn advanced_settings<R: FnMut() -> Result<String>, W: Write>(
    draft: &mut Playback,
    ui: &mut Ui<R, W>,
    cancel: &AtomicBool,
) -> Result<()> {
    ui.say("\n4. Audio settings\nPiAir detects audio automatically.\n1. Continue [default]\n2. Advanced audio and volume settings")?;
    if ui.choice("Choose:", 2, Some(1))? == 2 {
        configure_activity(draft, ui, cancel)?;
        configure_volume(draft, ui)?;
    }
    Ok(())
}

fn configure_activity<R: FnMut() -> Result<String>, W: Write>(
    draft: &mut Playback,
    ui: &mut Ui<R, W>,
    cancel: &AtomicBool,
) -> Result<()> {
    loop {
        ui.say(format!("\nAdvanced audio detection\nLouder levels are closer to 0 dBFS.\nThreshold: {} dBFS\nStart after: {} ms of sustained audio\nStop after: {} ms of silence\n1. Keep current/default settings\n2. Adjust manually\n3. Measure stopped source and music; suggest threshold",draft.threshold_dbfs,draft.start_duration_ms,draft.stop_duration_ms))?;
        match ui.choice("Choose:", 3, Some(1))? {
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
        ui.say(format!("Speaker volume: {}\n1. Keep current configuration\n2. Keep speaker volume unchanged\n3. Set an explicit volume",volume_label(draft.volume_db)))?;
        match ui.choice("Choose volume:", 3, Some(1))? {
            1 => return Ok(()),
            2 => {
                draft.volume_db = None;
                return Ok(());
            }
            _ => {
                let value: i32 = ui.number("Speaker volume in dB (-60 to -10):")?;
                if !(-60..=-10).contains(&value) {
                    ui.say("Volume out of range.")?;
                    continue;
                }
                if ui.confirm(&format!("Set speaker volume to {value} dB?\nThe speaker may keep this volume after playback."))?{draft.volume_db=Some(value);return Ok(())}
            }
        }
    }
}
fn test_audio_requested<R: FnMut() -> Result<String>, W: Write>(ui: &mut Ui<R, W>) -> Result<bool> {
    ui.say("\n7. Optional audio test\nPlay a recording on your speaker. This tests the speaker connection, not your audio input.\n1. Play test recording (49 seconds)\n2. Skip test [default]")?;
    Ok(ui.choice("Choose:", 2, Some(2))? == 1)
}
fn playback_troubleshooting<R: FnMut() -> Result<String>, W: Write>(
    ui: &mut Ui<R, W>,
) -> Result<()> {
    ui.say("Audio test: Failed.\nCheck the speaker volume and try the test again.")
}

fn playback_feedback<R: FnMut() -> Result<String>, W: Write>(
    result: Result<()>,
    ui: &mut Ui<R, W>,
    cancel: &AtomicBool,
) -> Result<TestOutcome> {
    playback_feedback_details(result, ui, cancel, tracing::enabled!(tracing::Level::DEBUG))
}
fn playback_feedback_details<R: FnMut() -> Result<String>, W: Write>(
    result: Result<()>,
    ui: &mut Ui<R, W>,
    cancel: &AtomicBool,
    detailed: bool,
) -> Result<TestOutcome> {
    if result
        .as_ref()
        .is_err_and(|e| e.is::<super::lifecycle::CleanupFailure>())
    {
        return Err(result.unwrap_err()).context(FatalPlayback);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled.into());
    }
    if let Err(e) = result {
        tracing::debug!(error = %format!("{e:#}"), "audio test failed");
        let message = e.downcast_ref::<super::playback_error::Stage>().map_or(
            "PiAir couldn't prepare the audio test.\nCheck your settings and try again.",
            |stage| stage.message(),
        );
        if let Some(permission) = e.downcast_ref::<super::wire::PtpPermissionDenied>() {
            ui.say(permission.to_string())?;
        } else {
            ui.say(message)?;
        }
        if detailed {
            ui.say(format!("Details: {e:#}"))?;
        } else {
            ui.say("For details, run piair -v setup (or -vv for more diagnostics).")?;
        }
        ui.say("Audio test: Failed.")?;
        return Ok(TestOutcome::Failed);
    }
    ui.say("Did you hear the audio?\n1. Yes, audio works\n2. No, show troubleshooting guidance\n3. Skip confirmation")?;
    let outcome = match ui.choice("Choose:", 3, None)? {
        1 => TestOutcome::Passed,
        2 => {
            playback_troubleshooting(ui)?;
            return Ok(TestOutcome::Failed);
        }
        _ => TestOutcome::Skipped,
    };
    ui.say(format!("Audio test: {}.", outcome.label()))?;
    Ok(outcome)
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
        Some(Authentication::StoredHap) => "Saved pairing",
        Some(Authentication::OwnToneTransient) => "Without pairing",
        None => "Not configured",
    }
}
fn volume_label(volume: Option<i32>) -> String {
    volume.map_or_else(
        || "Keep speaker volume unchanged".to_owned(),
        |v| format!("{v} dB (may persist)"),
    )
}
fn summary<R: FnMut() -> Result<String>, W: Write>(
    old: &Playback,
    new: &Playback,
    ui: &mut Ui<R, W>,
) -> Result<()> {
    ui.say("\nAdvanced playback settings (* = changed)")?;
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
            "Speaker",
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
            "Connection",
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
            "Speaker volume",
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
    ui.say("\n6. Optional automatic startup")?;
    let service = match inspect(loaded) {
        Ok(s) => s,
        Err(e) => {
            ui.say(format!("Automatic activation unavailable: {e}\nSetup can finish successfully without a service.\nManual procedure: docs/SYSTEMD.md."))?;
            return Ok(());
        }
    };
    tracing::debug!(status = %service.status, "verified installed service");
    ui.say("Start PiAir automatically when Raspberry Pi boots?\n1. Yes\n2. No [default]")?;
    if ui.choice("Choose:", 2, Some(2))? == 2 {
        return Ok(());
    }
    if !ui.confirm(&format!(
        "Enable automatic startup and start PiAir now?\nAccount: {}\nSettings: {}",
        service.user,
        loaded.path.display()
    ))? {
        return Ok(());
    }
    match activate(loaded, &service) {
        Ok(()) => {
            journal.push(format!("Enabled and started {}", service.unit));
            ui.say("Automatic startup enabled. PiAir started.")?;
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
