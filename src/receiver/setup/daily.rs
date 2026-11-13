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
fn stop_for_action<R: FnMut() -> Result<String>, W: Write>(
    loaded: &Loaded,
    ui: &mut Ui<R, W>,
) -> Result<Option<bool>> {
    let running = match service::observe(loaded) {
        Ok(s) if s.active == "active" && s.pid != 0 => true,
        Ok(s) if matches!(s.active.as_str(), "inactive" | "failed") && s.pid == 0 => false,
        Ok(_) => anyhow::bail!("service is transitioning; wait and retry"),
        // An unavailable or custom service is never controlled. The kernel lock
        // below still prevents changing anything under a manual runtime.
        Err(_) => false,
    };
    confirm_stop(ui, running, || service::control(loaded, "stop"))
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
fn restart_offer<R: FnMut() -> Result<String>, W: Write>(
    loaded: &Loaded,
    ui: &mut Ui<R, W>,
) -> Result<()> {
    if ui.confirm("Start PiAir now?")? {
        service::control(loaded, "start")?;
    } else {
        ui.say("PiAir remains stopped. Saved settings will be used next time it starts.")?;
    }
    Ok(())
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
        let Some(was_running) = stop_for_action(loaded, ui)? else {
            return Ok(());
        };
        let instance = runtime::InstanceLock::acquire()?;
        complete(loaded)?;
        ensure!(
            allowlist::snapshot(&loaded.path)? == reviewed,
            "configuration changed while stopping; retry"
        );
        let mut journal = Vec::new();
        approve(loaded, &id, ui, &mut journal)?;
        for entry in journal {
            ui.say(format!(
                "{entry}. This approval remains saved if later steps are cancelled."
            ))?;
        }
        let baseline = allowlist::snapshot(&loaded.path)?;
        // Preserve explicit authentication intent. Never infer or fall back.
        let auth = loaded
            .config
            .playback
            .authentication
            .context("Run piair setup to configure authentication")?;
        let method = match auth {
            Authentication::StoredHap => Method::StoredHap,
            Authentication::OwnToneTransient => Method::Transient,
        };
        let mut selected = super::super::validate_selection(loaded, &id, method, &rows)
            .context("Receiver authentication is not configured or compatible. Run piair setup; no fallback attempted.")?;
        if auth == Authentication::StoredHap {
            super::super::hap_credential(&store, &selected).context(
                "No validated saved credentials. Run piair setup to configure authentication.",
            )?;
            selected.method = Method::SetupCode;
        } else {
            ensure!(ui.confirm("Use the configured transient authentication for this receiver?\nThis does not establish persistent verified identity.")?, Cancelled);
        }
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
        drop(instance);
        *loaded = config::load(Some(loaded.path.clone()))?;
        if was_running {
            restart_offer(loaded, ui)?;
        }
        return Ok(());
    }
}
fn setup_child(loaded: &Loaded) -> Result<()> {
    let mut command = std::process::Command::new(std::env::current_exe()?);
    // An absent default config is valid for setup; making it explicit would
    // turn it into a missing-file error before the wizard could create it.
    if loaded.source == "file" || loaded.path.exists() {
        command.arg("--config").arg(&loaded.path);
    }
    let status = command.arg("setup").status()?;
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
                        if stop_for_action(&loaded, &mut ui)?.is_some() {
                            setup_child(&loaded)?;
                        }
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
                    if let Some(was_running) = stop_for_action(&loaded, &mut ui)? {
                        let instance = runtime::InstanceLock::acquire()?;
                        let result =
                            test_audio(&loaded, &loaded.config.playback, &mut ui, cancel.clone());
                        drop(instance);
                        result?;
                        if was_running {
                            restart_offer(&loaded, &mut ui)?;
                        }
                    }
                }
                5 => {
                    ui.say("1. Run setup\n2. View saved playback settings\nQ. Back")?;
                    match ui.choice("Choose:", 2, None)? {
                        1 => {
                            if let Some(was_running) = stop_for_action(&loaded, &mut ui)? {
                                setup_child(&loaded)?;
                                let refreshed = config::load(Some(loaded.path.clone()))?;
                                if was_running {
                                    restart_offer(&refreshed, &mut ui)?;
                                }
                            }
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
}
