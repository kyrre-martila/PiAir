//! Daily operations compose the setup wizard and the verified installed service.
use super::*;

fn complete(loaded: &Loaded) -> Result<()> {
    runtime::Runtime::prepare(loaded, loaded.config.playback.effective()?)?;
    Ok(())
}
fn activity(service: &service::Service) -> &'static str {
    match service.active.as_str() {
        "active" if service.pid != 0 => "Running - activity unknown",
        "inactive" => "Stopped",
        "failed" => "Failed",
        _ => "Unavailable (service transitioning or no live process)",
    }
}
fn boot_label(state: &str) -> String {
    match state {
        "enabled" => "Yes".into(),
        "disabled" | "masked" | "enabled-runtime" | "masked-runtime" => format!("No ({state})"),
        _ => format!("Unknown ({})", clean(state)),
    }
}
fn status_text(loaded: &Loaded, detailed: bool) -> String {
    let state = match service::observe(loaded) {
        Ok(s) => format!(
            "Service: {}\nStarts at boot: {}\nMain PID: {}\nService result: {}",
            activity(&s),
            boot_label(&s.boot),
            s.pid,
            clean(&s.result)
        ),
        Err(e) => format!("Service: Unavailable\n{e:#}"),
    };
    let selected = Store::new(&loaded.path).and_then(|s| s.selection());
    let receiver = match selected {
        Ok(Some(s)) => format!(
            "{} ({})",
            clean(s.display_name.as_deref().unwrap_or("Saved receiver")),
            s.id
        ),
        Ok(None) => "Not selected".into(),
        Err(e) => format!("Unavailable: {e}"),
    };
    let configuration = match complete(loaded) {
        Ok(()) => "Complete".into(),
        Err(e) if detailed => format!("Incomplete: {e}\nRun piair setup to configure PiAir."),
        Err(_) => "Incomplete. Run piair setup to configure PiAir.".into(),
    };
    format!("PiAir\n\n{state}\nReceiver: {receiver}\nAudio input: {}\nConfiguration: {} ({})\n{configuration}\nPlayback: activity unknown; audible output unverified",
        clean(loaded.config.playback.capture_device.as_deref().unwrap_or("Not configured")),
        loaded.path.display(), loaded.source)
}
pub(crate) fn daily_status(loaded: &Loaded) -> Result<()> {
    println!("{}", status_text(loaded, true));
    Ok(())
}
pub(crate) fn daily_control(loaded: &Loaded, action: &str) -> Result<()> {
    service::control(loaded, action)
}
pub(crate) fn daily_logs(loaded: &Loaded, follow: bool, detailed: bool) -> Result<()> {
    let cancel = runtime::signal_cancellation()?;
    service::logs(loaded, follow, detailed, &cancel)
}
fn service_running(loaded: &Loaded) -> Result<bool> {
    match service::observe(loaded) {
        Ok(s) if s.active == "active" && s.pid != 0 => Ok(true),
        Ok(s) if matches!(s.active.as_str(), "inactive" | "failed") && s.pid == 0 => Ok(false),
        Ok(_) => anyhow::bail!("service is transitioning; wait and retry"),
        // An unavailable service is never controlled. The runtime lock still
        // prevents modifying settings underneath a manual instance.
        Err(_) => Ok(false),
    }
}
fn confirm_stop<R: FnMut() -> Result<String>, W: Write>(
    ui: &mut Ui<R, W>,
    running: bool,
    stop: impl FnOnce() -> Result<()>,
) -> Result<Option<bool>> {
    if running {
        if !ui.confirm("PiAir must stop before continuing.\nStop PiAir and continue?")? {
            return Ok(None);
        }
        stop()?;
    }
    Ok(Some(running))
}
fn safe_saved_settings(path: &std::path::Path) -> Result<Loaded> {
    let saved = config::load(Some(path.to_owned()))?;
    complete(&saved)?;
    // No prompt or service activation while another runtime owns the lock.
    drop(runtime::InstanceLock::acquire()?);
    Ok(saved)
}
fn stopped_guidance<R: FnMut() -> Result<String>, W: Write>(ui: &mut Ui<R, W>) -> Result<()> {
    ui.say("PiAir remains stopped. When ready, run piair start.\nUse the same --config path if you supplied one.")
}
// All three daily actions use this path, after their resources/locks are dropped.
fn finish_action<R: FnMut() -> Result<String>, W: Write, T>(
    ui: &mut Ui<R, W>,
    was_running: bool,
    cancel: &AtomicBool,
    outcome: Result<()>,
    check: impl FnOnce() -> Result<T>,
    start: impl FnOnce(T) -> Result<()>,
) -> Result<()> {
    if !was_running {
        return outcome;
    }
    if let Err(error) = &outcome {
        if error.is::<Cancelled>() {
            ui.say("Action cancelled.")?;
        } else {
            ui.say(format!("Action failed: {error:#}"))?;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return stopped_guidance(ui);
    }
    if outcome
        .as_ref()
        .is_err_and(|e| e.is::<super::super::lifecycle::CleanupFailure>())
    {
        ui.say("Cleanup could not be verified. Resolve the reported cleanup failure before starting PiAir.")?;
        return stopped_guidance(ui);
    }
    let saved = match check() {
        Ok(saved) => saved,
        Err(error) => {
            ui.say(format!("Cannot safely start PiAir: {error:#}\nReview saved settings with piair status and piair setup.\nA manual runtime must be stopped manually; no settings were rolled back by recovery."))?;
            return stopped_guidance(ui);
        }
    };
    // A signal during validation must not lead to a new prompt or activation.
    if cancel.load(Ordering::Relaxed) {
        return stopped_guidance(ui);
    }
    match ui.confirm("Saved settings are consistent. Start PiAir again?") {
        Ok(true) if !cancel.load(Ordering::Relaxed) => {
            if let Err(error) = start(saved) {
                stopped_guidance(ui)?;
                return Err(error);
            }
            Ok(())
        }
        Ok(_) => stopped_guidance(ui),
        Err(error) => {
            stopped_guidance(ui)?;
            if error.is::<Cancelled>() {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}
fn stopped_action<R: FnMut() -> Result<String>, W: Write>(
    loaded: &mut Loaded,
    ui: &mut Ui<R, W>,
    cancel: &AtomicBool,
    action: impl FnOnce(&mut Loaded, &mut Ui<R, W>) -> Result<()>,
) -> Result<()> {
    let was_running = service_running(loaded)?;
    let outcome = match confirm_stop(ui, was_running, || service::control(loaded, "stop")) {
        Ok(None) => return Ok(()),
        Ok(Some(_)) => action(loaded, ui),
        Err(error) => {
            // systemctl can stop successfully and then fail the lock-release
            // check. Recover only when the same user's unit is actually stopped.
            let stopped = was_running
                && !error.is::<Cancelled>()
                && service::observe(loaded).is_ok_and(|s| {
                    matches!(s.active.as_str(), "inactive" | "failed") && s.pid == 0
                });
            if !stopped {
                return Err(error);
            }
            Err(error)
        }
    };
    // Setup may have explicitly activated the existing service itself. Do not
    // describe it as stopped or request a second activation in that case.
    if was_running && service::observe(loaded).is_ok_and(|s| s.active == "active" && s.pid != 0) {
        ui.say("PiAir is already running. No recovery start is needed.")?;
        return outcome;
    }
    finish_action(
        ui,
        was_running,
        cancel,
        outcome,
        || safe_saved_settings(&loaded.path),
        // Confirmation may take time: reload/check again immediately before
        // requesting activation, rather than using the pre-prompt snapshot.
        |saved| service::control(&safe_saved_settings(&saved.path)?, "start"),
    )
}
fn receiver_preflight(
    loaded: &Loaded,
    id: &str,
    rows: &[Receiver],
) -> Result<super::super::Selection> {
    complete(loaded)?;
    let auth = loaded
        .config
        .playback
        .authentication
        .context("Run piair setup to configure authentication")?;
    let method = match auth {
        Authentication::StoredHap => Method::StoredHap,
        Authentication::OwnToneTransient => Method::Transient,
    };
    let mut selected = super::super::validate_receiver_observation(loaded, id, method, rows)
        .context("Receiver is not compatible. Run piair setup; no fallback attempted.")?;
    super::super::wav_authentication(
        &Store::new(&loaded.path)?,
        &selected,
        auth == Authentication::OwnToneTransient,
    )
    .context(
        "No validated authentication for this receiver. Run piair setup; no fallback attempted.",
    )?;
    let required = (1u64 << 9) | (1u64 << 19) | (1u64 << 41);
    ensure!(
        selected.capabilities.features & required == required,
        "receiver cannot provide the required audio playback; no fallback attempted"
    );
    if auth == Authentication::StoredHap {
        selected.method = Method::SetupCode;
    }
    Ok(selected)
}
fn choose_daily_receiver<R: FnMut() -> Result<String>, W: Write>(
    loaded: &Loaded,
    rows: &[Receiver],
    ui: &mut Ui<R, W>,
) -> Result<Option<String>> {
    ui.say("Select AirPlay receiver:")?;
    for (i, row) in rows.iter().enumerate() {
        let marker = if loaded.config.playback.receiver_id.as_deref() == Some(&row.id) {
            " (selected)"
        } else {
            ""
        };
        ui.say(format!("{}. {}{marker}", i + 1, clean(&row.name)))?;
    }
    ui.say(format!("{}. Search again\nQ. Back", rows.len() + 1))?;
    let n = ui.choice("Choose:", rows.len() + 1, None)?;
    if n == rows.len() + 1 {
        return Ok(None);
    }
    let row = &rows[n - 1];
    super::super::exact_id(&row.id)?;
    ensure!(
        rows.iter().filter(|r| r.id == row.id).count() == 1,
        "receiver identity is ambiguous; search again"
    );
    ui.say(format!(
        "Receiver: {}\nCanonical identity: {}\nAdvertised names do not verify identity.",
        clean(&row.name),
        row.id
    ))?;
    Ok(Some(row.id.clone()))
}
fn receiver<R: FnMut() -> Result<String>, W: Write>(
    loaded: &mut Loaded,
    ui: &mut Ui<R, W>,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    loop {
        let reviewed = allowlist::snapshot(&loaded.path)?;
        let rows = discover_receivers(loaded, ui, cancel)?;
        let Some(id) = choose_daily_receiver(loaded, &rows, ui)? else {
            continue;
        };
        let store = Store::new(&loaded.path)?;
        let previous = store
            .selection()?
            .context("No saved selection. Run piair setup first.")?;
        if previous.id == id && loaded.config.playback.receiver_id.as_deref() == Some(&id) {
            ui.say("This receiver is already selected. No changes made.")?;
            return Ok(());
        }
        if !ui.confirm(&format!("Change the saved receiver to exactly {id}?"))? {
            return Ok(());
        }
        ensure!(
            allowlist::snapshot(&loaded.path)? == reviewed,
            "configuration changed during receiver review; retry"
        );
        receiver_preflight(loaded, &id, &rows)?;
        if loaded.config.playback.authentication == Some(Authentication::OwnToneTransient) {
            ensure!(ui.confirm("Use the configured transient authentication for this receiver?\nThis does not establish persistent verified identity.")?, Cancelled);
        }
        return stopped_action(loaded, ui, cancel, |loaded, ui| {
            let _instance = runtime::InstanceLock::acquire()?;
            complete(loaded)?;
            ensure!(
                allowlist::snapshot(&loaded.path)? == reviewed,
                "configuration changed while stopping; retry"
            );
            let mut journal = Vec::new();
            let name = &rows
                .iter()
                .find(|r| r.id == id)
                .context("receiver missing")?
                .name;
            approve_named(loaded, &id, name, ui, &mut journal)?;
            for entry in journal {
                ui.say(format!(
                    "{entry}. This approval remains saved if later steps are cancelled."
                ))?;
            }
            let baseline = allowlist::snapshot(&loaded.path)?;
            // Recheck credentials and endpoint authorization after stopping and
            // any explicit approval; preflight never grants authorization.
            let selected = receiver_preflight(loaded, &id, &rows)?;
            super::super::validate_selection(loaded, &id, selected.method, &rows)?;
            ensure!(!cancel.load(Ordering::Relaxed), Cancelled);
            store.change_with_playback(&selected, &previous, || {
                config::save_playback_for_selection(
                    loaded,
                    id.clone(),
                    playback_args(&loaded.config.playback),
                    &baseline,
                )
            })?;
            ui.say(format!(
                "Receiver changed to {}.",
                clean(
                    &rows
                        .iter()
                        .find(|r| r.id == id)
                        .expect("chosen receiver")
                        .name
                )
            ))?;
            Ok(())
        });
    }
}
fn setup_child(loaded: &Loaded, cancel: &AtomicBool) -> Result<()> {
    let mut command = std::process::Command::new(std::env::current_exe()?);
    // An absent default config is valid for setup; making it explicit would
    // turn it into a missing-file error before the wizard could create it.
    if loaded.source == "file" || loaded.path.exists() {
        command.arg("--config").arg(&loaded.path);
    }
    let mut child = command.arg("setup").spawn()?;
    let mut interrupted = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancel.load(Ordering::Relaxed) && !interrupted {
            // Signal only the wizard child we spawned, never a manual runtime.
            rustix::process::kill_process(
                rustix::process::Pid::from_child(&child),
                rustix::process::Signal::INT,
            )?;
            interrupted = true;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    ensure!(
        status.success(),
        "Setup failed; inspect saved settings before starting PiAir"
    );
    Ok(())
}

fn menu_text(running: bool) -> String {
    format!("\n1. Status\n2. {} PiAir\n3. Select AirPlay receiver\n4. Test audio (49 seconds)\n5. Settings\n6. Logs\n\nQ. Exit", if running { "Stop" } else { "Start" })
}
pub(crate) fn daily(mut loaded: Loaded) -> Result<()> {
    ensure!(io::stdin().is_terminal() && io::stdout().is_terminal(),
        "piair without a command requires an interactive terminal; use piair status, start, stop, logs or --help in scripts");
    let cancel = runtime::signal_cancellation()?;
    let mut ui = Ui {
        read: || ui::terminal_line(&cancel),
        out: io::stdout(),
    };
    loop {
        loaded = config::load(Some(loaded.path.clone())).or_else(|e| {
            if loaded.source != "file" && !loaded.path.exists() {
                config::load(None)
            } else {
                Err(e)
            }
        })?;
        ui.say(status_text(&loaded, false))?;
        let running = service::observe(&loaded).is_ok_and(|s| s.active == "active" && s.pid != 0);
        let incomplete = complete(&loaded).is_err();
        if incomplete {
            ui.say("\nPlayback configuration needs setup.\n1. Run piair setup\n2. Status\n3. Logs\nQ. Exit")?;
        } else {
            ui.say(menu_text(running))?;
        }
        let choice = match ui.choice("Choose:", if incomplete { 3 } else { 6 }, None) {
            Ok(n) => n,
            Err(e) if e.is::<Cancelled>() => return Ok(()),
            Err(e) => return Err(e),
        };
        let result = (|| -> Result<()> {
            if incomplete {
                match choice {
                    1 => {
                        stopped_action(&mut loaded, &mut ui, &cancel, |loaded, _| {
                            setup_child(loaded, &cancel)
                        })?;
                    }
                    2 => daily_status(&loaded)?,
                    _ => service::logs(&loaded, false, false, &cancel)?,
                }
                return Ok(());
            }
            match choice {
                1 => daily_status(&loaded)?,
                2 => service::control(&loaded, if running { "stop" } else { "start" })?,
                3 => receiver(&mut loaded, &mut ui, &cancel)?,
                4 => {
                    complete(&loaded)?;
                    if !test_audio_requested(&mut ui)? {
                        return Ok(());
                    }
                    let path = resources::locate().context("Test recording or LICENSE.md is missing. Install both in /usr/local/share/piair/audio or set PIAIR_AUDIO_DATA_DIR. PiAir was not stopped.")?;
                    // Parse before stopping too: a missing, unreadable or invalid
                    // WAV must not interrupt working playback.
                    super::super::wav::Playback::open(&path, 49)?;
                    stopped_action(&mut loaded, &mut ui, &cancel, |loaded, ui| {
                        let _instance = runtime::InstanceLock::acquire()?;
                        play_test_audio(loaded, &loaded.config.playback, ui, cancel.clone())
                    })?;
                }
                5 => {
                    ui.say("1. Run setup\n2. View saved playback settings\nQ. Back")?;
                    match ui.choice("Choose:", 2, None)? {
                        1 => {
                            stopped_action(&mut loaded, &mut ui, &cancel, |loaded, _| {
                                setup_child(loaded, &cancel)
                            })?;
                        }
                        _ => ui.say(toml::to_string_pretty(&loaded.config.playback)?)?,
                    }
                }
                _ => service::logs(&loaded, false, false, &cancel)?,
            }
            Ok(())
        })();
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Err(e) = result {
            if !e.is::<Cancelled>() {
                ui.say(format!("Action failed: {e:#}"))?;
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dynamic_single_toggle_and_unknown_activity() {
        assert!(menu_text(true).contains("2. Stop PiAir"));
        assert!(menu_text(false).contains("2. Start PiAir"));
        let mut s = service::Service {
            unit: "piair@pi.service".into(),
            status: "active".into(),
            home: "/home/pi".into(),
            user: "pi".into(),
            active: "active".into(),
            boot: "disabled".into(),
            pid: 123,
            result: "success".into(),
        };
        assert_eq!(activity(&s), "Running - activity unknown");
        s.active = "inactive".into();
        s.pid = 0;
        assert_eq!(activity(&s), "Stopped");
        s.active = "failed".into();
        assert_eq!(activity(&s), "Failed");
        s.active = "active".into();
        assert!(activity(&s).starts_with("Unavailable"));
        assert_eq!(boot_label("enabled"), "Yes");
        assert_eq!(boot_label("disabled"), "No (disabled)");
        assert_eq!(boot_label("enabled-runtime"), "No (enabled-runtime)");
    }
    fn scripted(input: &str) -> Ui<impl FnMut() -> Result<String> + '_, Vec<u8>> {
        use std::io::BufRead;
        let mut input = input.as_bytes();
        Ui {
            read: move || {
                let mut line = String::new();
                input.read_line(&mut line)?;
                Ok(line)
            },
            out: Vec::new(),
        }
    }
    #[test]
    fn stopping_requires_confirmation_and_success_before_modification() {
        for answer in ["2\n", "\n"] {
            let mut stopped = false;
            assert_eq!(
                confirm_stop(&mut scripted(answer), true, || {
                    stopped = true;
                    Ok(())
                })
                .unwrap(),
                None
            );
            assert!(!stopped);
        }
        for answer in ["q\n", "", "1"] {
            assert!(confirm_stop(&mut scripted(answer), true, || panic!("must not stop")).is_err());
        }
        assert_eq!(
            confirm_stop(&mut scripted(""), false, || panic!("already stopped")).unwrap(),
            Some(false)
        );
        let mut stopped = false;
        assert_eq!(
            confirm_stop(&mut scripted("1\n"), true, || {
                stopped = true;
                Ok(())
            })
            .unwrap(),
            Some(true)
        );
        assert!(stopped);
        assert!(confirm_stop(&mut scripted("1\n"), true, || anyhow::bail!("denied")).is_err());
    }
    #[test]
    fn receiver_picker_marks_selected_retries_and_rescans_without_activation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        let mut loaded = config::load(Some(path)).unwrap();
        let id = "device:112233445566";
        loaded.config.playback.receiver_id = Some(id.into());
        let rows = vec![Receiver {
            id: id.into(),
            name: "Synthetic room".into(),
            services: Vec::new(),
        }];
        let mut ui = scripted("invalid\n1\n");
        assert_eq!(
            choose_daily_receiver(&loaded, &rows, &mut ui)
                .unwrap()
                .as_deref(),
            Some(id)
        );
        let text = String::from_utf8(ui.out).unwrap();
        assert!(text.contains("Synthetic room (selected)"));
        assert!(text.contains("Enter a number"));
        assert!(choose_daily_receiver(&loaded, &rows, &mut scripted("2\n"))
            .unwrap()
            .is_none());
        assert!(choose_daily_receiver(&loaded, &[], &mut scripted("1\n"))
            .unwrap()
            .is_none());
        assert!(choose_daily_receiver(&loaded, &rows, &mut scripted("q\n")).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    static RECOVERY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn working_settings() -> (tempfile::TempDir, Loaded) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "receiver_allowlist=['device:aabbccddeeff']\n[playback]\nreceiver_id='device:aabbccddeeff'\ncapture_device='synthetic-only'\nauthentication='own-tone-transient'\n").unwrap();
        let loaded = config::load(Some(path)).unwrap();
        let selected: super::super::super::Selection = toml::from_str("id='device:aabbccddeeff'\nmethod='transient'\naddresses=['127.0.0.1']\nport=9\n[capabilities]\nfeatures='0001420000080200'\nflags=0\n").unwrap();
        Store::new(&loaded.path).unwrap().select(&selected).unwrap();
        complete(&loaded).unwrap();
        (dir, loaded)
    }
    #[test]
    fn cancelled_receiver_authentication_wav_and_setup_failures_offer_confirmed_recovery() {
        let _guard = RECOVERY_LOCK.lock().unwrap();
        for failure in [
            "cancelled receiver change",
            "receiver authentication failed",
            "WAV test failed",
            "setup failed",
        ] {
            for answer in ["1\n", "2\n", "q\n", ""] {
                let (_dir, loaded) = working_settings();
                let before = std::fs::read(&loaded.path).unwrap();
                let selection = Store::new(&loaded.path).unwrap().selection().unwrap();
                let mut running = true;
                let mut actions = Vec::new();
                let was_running = confirm_stop(&mut scripted("1\n"), running, || {
                    actions.push("stop");
                    running = false;
                    Ok(())
                })
                .unwrap()
                .unwrap();
                assert!(!running);
                let outcome = if failure == "cancelled receiver change" {
                    Err(Cancelled.into())
                } else {
                    Err(anyhow::anyhow!(failure))
                };
                let mut ui = scripted(answer);
                finish_action(
                    &mut ui,
                    was_running,
                    &AtomicBool::new(false),
                    outcome,
                    || safe_saved_settings(&loaded.path),
                    |saved| {
                        assert_eq!(
                            saved.config.playback.receiver_id,
                            loaded.config.playback.receiver_id
                        );
                        actions.push("start");
                        running = true;
                        Ok(())
                    },
                )
                .unwrap();
                assert_eq!(running, answer == "1\n", "{failure}: {answer:?}");
                assert_eq!(
                    actions,
                    if running {
                        vec!["stop", "start"]
                    } else {
                        vec!["stop"]
                    }
                );
                assert_eq!(std::fs::read(&loaded.path).unwrap(), before);
                assert!(Store::new(&loaded.path).unwrap().selection().unwrap() == selection);
                let text = String::from_utf8(ui.out).unwrap();
                assert!(text.contains("Start PiAir again?"));
                if !running {
                    assert!(text.contains("PiAir remains stopped"));
                    assert!(text.contains("piair start"));
                }
            }
        }
    }
    #[test]
    fn changed_selection_blocks_recovery_without_rollback() {
        let _guard = RECOVERY_LOCK.lock().unwrap();
        let (_dir, loaded) = working_settings();
        let store = Store::new(&loaded.path).unwrap();
        let mut changed = store.selection().unwrap().unwrap();
        changed.id = "device:112233445566".into();
        store.select(&changed).unwrap();
        let mut ui = scripted("1\n");
        finish_action(
            &mut ui,
            true,
            &AtomicBool::new(false),
            Err(anyhow::anyhow!("partial save")),
            || safe_saved_settings(&loaded.path),
            |_| panic!("inconsistent settings must not start"),
        )
        .unwrap();
        let text = String::from_utf8(ui.out).unwrap();
        assert!(text.contains("Cannot safely start"));
        assert!(text.contains("piair setup"));
        assert!(!text.contains("Start PiAir again?"));
        assert!(store.selection().unwrap().as_ref() == Some(&changed));
    }
    #[test]
    fn ctrl_c_before_during_validation_or_confirmation_never_starts() {
        for phase in ["before", "validation", "confirmation"] {
            let cancel = AtomicBool::new(phase == "before");
            let mut ui = Ui {
                read: || {
                    assert_eq!(phase, "confirmation");
                    cancel.store(true, Ordering::Relaxed);
                    Ok("1\n".into())
                },
                out: Vec::new(),
            };
            finish_action(
                &mut ui,
                true,
                &cancel,
                Err(Cancelled.into()),
                || {
                    assert_ne!(phase, "before");
                    if phase == "validation" {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    Ok(())
                },
                |_| panic!("Ctrl+C must not start"),
            )
            .unwrap();
            assert!(String::from_utf8(ui.out)
                .unwrap()
                .contains("PiAir remains stopped"));
        }
    }
    #[test]
    fn previously_stopped_service_and_uncertain_cleanup_never_offer_start() {
        let mut ui = scripted("1\n");
        assert!(finish_action(
            &mut ui,
            false,
            &AtomicBool::new(false),
            Err(anyhow::anyhow!("setup failed")),
            || -> Result<()> { panic!("previously stopped") },
            |_| panic!("must not start")
        )
        .is_err());
        assert!(ui.out.is_empty());
        finish_action(
            &mut ui,
            true,
            &AtomicBool::new(false),
            Err(
                super::super::super::lifecycle::CleanupFailure(anyhow::anyhow!(
                    "uncertain teardown"
                ))
                .into(),
            ),
            || -> Result<()> { panic!("cleanup must be resolved first") },
            |_| panic!("must not start"),
        )
        .unwrap();
        assert!(String::from_utf8(ui.out)
            .unwrap()
            .contains("Cleanup could not be verified"));
    }
    #[test]
    fn manual_runtime_lock_blocks_recovery_without_killing_or_bypassing_it() {
        let _guard = RECOVERY_LOCK.lock().unwrap();
        let (_dir, loaded) = working_settings();
        let _instance = runtime::InstanceLock::acquire().unwrap();
        let mut ui = scripted("1\n");
        finish_action(
            &mut ui,
            true,
            &AtomicBool::new(false),
            Err(Cancelled.into()),
            || safe_saved_settings(&loaded.path),
            |_| panic!("manual runtime owns lock"),
        )
        .unwrap();
        assert!(runtime::InstanceLock::acquire().is_err());
        assert!(!String::from_utf8(ui.out)
            .unwrap()
            .contains("Start PiAir again?"));
    }
    #[test]
    fn unsupported_receiver_and_missing_authentication_fail_in_read_only_preflight() {
        let (_dir, loaded) = working_settings();
        let before = std::fs::read(&loaded.path).unwrap();
        let info = mdns_sd::ServiceInfo::new(
            "_airplay._tcp.local.",
            "New room",
            "room.local.",
            "127.0.0.2",
            7000,
            [
                ("deviceid", "11:22:33:44:55:66"),
                ("features", "0x80200,0x14200"),
                ("sf", "0x0"),
            ]
            .as_slice(),
        )
        .unwrap();
        let mut rows = vec![Receiver {
            id: "device:112233445566".into(),
            name: "New room".into(),
            services: vec![discovery::Advertisement::test_from_info(&info)],
        }];
        let id = rows[0].id.clone();
        // Even a compatible unapproved candidate gets no automatic approval.
        assert!(receiver_preflight(&loaded, &id, &rows).is_ok());
        assert!(!loaded.config.allows(&id));
        rows[0].services[0]
            .capabilities
            .insert("features".into(), "0x80200,0x200".into());
        assert!(receiver_preflight(&loaded, &id, &rows).is_err());
        assert_eq!(std::fs::read(&loaded.path).unwrap(), before);
        assert!(super::super::super::wav_authentication(
            &Store::new(&loaded.path).unwrap(),
            &Store::new(&loaded.path)
                .unwrap()
                .selection()
                .unwrap()
                .unwrap(),
            false
        )
        .is_err());
    }
    fn stored_working_settings() -> (tempfile::TempDir, Loaded) {
        let (dir, mut loaded) = working_settings();
        let store = Store::new(&loaded.path).unwrap();
        let mut selected = store.selection().unwrap().unwrap();
        selected.method = Method::SetupCode;
        store.select(&selected).unwrap();
        store
            .save(&super::super::super::store::Credential {
                version: 2,
                receiver_id: selected.id.clone(),
                advertised_pi: None,
                pairing_id: "fixture-receiver".into(),
                controller_id: "00000000-0000-4000-8000-000000000001".into(),
                seed: [11; 32],
                receiver_key: ed25519_dalek::SigningKey::from_bytes(&[22; 32])
                    .verifying_key()
                    .to_bytes(),
            })
            .unwrap();
        loaded.config.playback.authentication = Some(Authentication::StoredHap);
        std::fs::write(&loaded.path, toml::to_string(&loaded.config).unwrap()).unwrap();
        let saved = config::load(Some(loaded.path)).unwrap();
        complete(&saved).unwrap();
        (dir, saved)
    }
    #[test]
    fn missing_candidate_credentials_are_preflight_errors_and_revoked_saved_credentials_block_recovery(
    ) {
        let _guard = RECOVERY_LOCK.lock().unwrap();
        let (_dir, loaded) = stored_working_settings();
        let info = mdns_sd::ServiceInfo::new(
            "_airplay._tcp.local.",
            "New room",
            "room.local.",
            "127.0.0.2",
            7000,
            [
                ("deviceid", "11:22:33:44:55:66"),
                ("features", "0x80200,0x14200"),
                ("sf", "0x0"),
            ]
            .as_slice(),
        )
        .unwrap();
        let rows = vec![Receiver {
            id: "device:112233445566".into(),
            name: "New room".into(),
            services: vec![discovery::Advertisement::test_from_info(&info)],
        }];
        let failure = receiver_preflight(&loaded, &rows[0].id, &rows)
            .err()
            .expect("missing credentials");
        assert!(format!("{failure:#}").contains("no automatic pairing"));
        let mut ui = scripted("1\n");
        // The same validation can fail after stop if credentials were revoked
        // during the stop operation. The previous receiver remains usable.
        let mut started = false;
        finish_action(
            &mut ui,
            true,
            &AtomicBool::new(false),
            Err(failure),
            || safe_saved_settings(&loaded.path),
            |_| {
                started = true;
                Ok(())
            },
        )
        .unwrap();
        assert!(started);
        Store::new(&loaded.path)
            .unwrap()
            .remove("device:aabbccddeeff")
            .unwrap();
        let mut ui = scripted("1\n");
        finish_action(
            &mut ui,
            true,
            &AtomicBool::new(false),
            Err(Cancelled.into()),
            || safe_saved_settings(&loaded.path),
            |_| panic!("saved credentials were revoked"),
        )
        .unwrap();
        assert!(!String::from_utf8(ui.out)
            .unwrap()
            .contains("Start PiAir again?"));
    }
}
