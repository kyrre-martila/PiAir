//! Fixed, opt-in transient recording service. Never changes the installed unit.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
};

pub(super) fn fingerprint(path: &Path) -> Result<String> {
    let snapshot = allowlist::snapshot(path)?.context("Save configuration before testing")?;
    // Include identity and metadata: replacement with identical bytes is a change too.
    Ok(format!(
        "{:x}",
        Sha256::digest(format!("{snapshot:?}").as_bytes())
    ))
}
pub(super) fn check_saved(loaded: &Loaded, saved: &str) -> Result<()> {
    ensure!(fingerprint(&loaded.path)? == saved, "Configuration changed after saving. Review it and rerun the test; saved settings and credentials are retained.");
    let fresh = config::load(Some(loaded.path.clone()))?;
    ensure!(
        toml::to_string(&fresh.config)? == toml::to_string(&loaded.config)?,
        "Configuration changed while loading; review settings before testing"
    );
    ensure!(
        fingerprint(&loaded.path)? == saved,
        "Configuration changed while loading; review settings before testing"
    );
    Ok(())
}

/// CLI authorization is deliberately narrow: no file, receiver, volume, executable,
/// duration or arbitrary systemd property can be supplied by the caller.
pub(crate) fn worker(loaded: &Loaded, saved: &str) -> Result<()> {
    ensure!(
        rustix::process::geteuid().as_raw() != 0,
        "Recording test must run as the ordinary account"
    );
    let _installed = service::require_installed_executable()?;
    let _instance = runtime::InstanceLock::acquire()?;
    // Serialize ordinary config writers during this invocation. External edits
    // before acquisition are rejected by the fingerprint below.
    let directory = std::fs::File::open(loaded.path.parent().context("missing config directory")?)?;
    directory
        .try_lock()
        .context("Configuration is being edited; try again")?;
    check_saved(loaded, saved)?;
    let _service = service::inspect_current(loaded)?;
    let path = resources::locate_in(["/usr/local/share/piair/audio".into()])
        .context("Recording or LICENSE.md missing; reinstall PiAir")?;
    let cancel = runtime::signal_cancellation()?;
    check_saved(loaded, saved)?;
    recording_operation(loaded, &path, |id, transient, wav, volume| {
        super::super::ptp_operation_cancellable(
            loaded,
            id,
            transient,
            10,
            Some((wav, volume)),
            None,
            Some(cancel.clone()),
        )
    })?;
    ensure!(!cancel.load(Ordering::Relaxed), "Recording cancelled");
    Ok(())
}
fn recording_operation(
    loaded: &Loaded,
    path: &Path,
    send: impl FnOnce(&str, bool, super::super::wav::Playback, Option<i32>) -> Result<()>,
) -> Result<()> {
    let playback = &loaded.config.playback;
    let id = playback
        .receiver_id
        .as_deref()
        .context("No saved receiver")?;
    let authentication = playback
        .authentication
        .context("No saved authentication method")?;
    let wav = super::super::wav::Playback::open(path, 49)?;
    send(
        id,
        authentication == Authentication::OwnToneTransient,
        wav,
        None,
    )
}

#[derive(Debug)]
struct Teardown;
impl std::fmt::Display for Teardown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Temporary recording teardown could not be verified")
    }
}
impl std::error::Error for Teardown {}

trait Backend {
    fn inspect(&mut self) -> Result<bool>;
    fn validate(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn play(&mut self, cancelled: &mut dyn FnMut() -> Result<bool>) -> Result<()>;
    fn restore(&mut self) -> Result<()>;
}

#[derive(Debug)]
pub(super) struct Report {
    pub safe_to_restore: bool,
    pub outcome: TestOutcome,
}
impl Report {
    fn skipped() -> Self {
        Self {
            safe_to_restore: true,
            outcome: TestOutcome::Skipped,
        }
    }
}

pub(super) fn offer<R: FnMut() -> Result<String>, W: Write>(
    loaded: &Loaded,
    saved: &str,
    ui: &mut Ui<R, W>,
    cancel: &AtomicBool,
) -> Result<Report> {
    workflow(
        &mut System {
            loaded,
            saved,
            cancel,
        },
        ui,
        cancel,
    )
}
fn workflow<R: FnMut() -> Result<String>, W: Write>(
    backend: &mut impl Backend,
    ui: &mut Ui<R, W>,
    cancel: &AtomicBool,
) -> Result<Report> {
    if !test_audio_requested(ui)? {
        return Ok(Report::skipped());
    }
    let running = match backend.validate().and_then(|()| backend.inspect()) {
        Ok(running) => running,
        Err(e) => {
            ui.say(format!("Recording test unavailable: {e:#}\nSettings are saved. Continue without testing; see docs/SYSTEMD.md."))?;
            return Ok(Report::skipped());
        }
    };
    if running {
        if !ui
            .confirm("Stop PiAir to play the test recording? You can start it again afterward.")?
        {
            return Ok(Report::skipped());
        }
        if let Err(e) = backend.stop() {
            ui.say(format!("Could not safely stop automatic playback: {e:#}\nTest skipped. Inspect piair status; nothing will restart automatically."))?;
            return Ok(Report::skipped());
        }
    }
    ui.say("Playing the test recording at the speaker’s current volume.\nApprove system authorization if asked. Enter q or press Ctrl+C to cancel.")?;
    let result = backend.play(&mut || {
        if cancel.load(Ordering::Relaxed) {
            return Ok(true);
        }
        use rustix::event::{poll, PollFd, PollFlags, Timespec};
        let stdin = io::stdin();
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        match poll(
            &mut fds,
            Some(&Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            }),
        ) {
            Ok(0) | Err(rustix::io::Errno::INTR) => Ok(false),
            Err(e) => Err(e.into()),
            _ => match ui.line("Enter q to cancel:") {
                Ok(_) => Ok(false),
                Err(e) if e.is::<Cancelled>() => Err(e),
                Err(e) => Err(e),
            },
        }
    });
    if result.as_ref().is_err_and(|e| e.is::<Teardown>()) {
        ui.say(format!("{:#}\nAutomatic playback will not be restarted. Saved settings and credentials are retained.", result.unwrap_err()))?;
        return Ok(Report {
            safe_to_restore: false,
            outcome: TestOutcome::Failed,
        });
    }
    // Cancellation never triggers a restart or another prompt.
    if cancel.load(Ordering::Relaxed) || result.as_ref().is_err_and(|e| e.is::<Cancelled>()) {
        ui.say("Recording cancelled. Automatic playback has not been restarted. Use piair status, then piair start when ready.")?;
        return Err(Cancelled.into());
    }
    let outcome = match result {
        Ok(()) => playback_feedback(Ok(()), ui, cancel),
        Err(e) => {
            ui.say(setup_problem(
                &e,
                &format!("Audio test: Failed. {e}\nCheck piair logs, speaker availability and system authorization."),
                tracing::enabled!(tracing::Level::DEBUG),
            ))?;
            Ok(TestOutcome::Failed)
        }
    };
    // q/EOF during listening confirmation must not cause a restart either.
    let outcome = outcome?;
    if running && ui.confirm("Start PiAir again?")? {
        if let Err(e) = backend.restore() {
            ui.say(format!("Could not restore playback: {e:#}\nInspect piair status, then explicitly run piair start."))?;
        }
    }
    Ok(Report {
        safe_to_restore: true,
        outcome,
    })
}

struct System<'a> {
    loaded: &'a Loaded,
    saved: &'a str,
    cancel: &'a AtomicBool,
}
impl Backend for System<'_> {
    fn validate(&mut self) -> Result<()> {
        check_saved(self.loaded, self.saved)?;
        let _installed = service::require_installed_executable()?;
        let service = service::inspect_current(self.loaded)?;
        service::no_other_services(&service.unit)?;
        let path = resources::locate_in(["/usr/local/share/piair/audio".into()])
            .context("Recording or LICENSE.md missing; reinstall PiAir")?;
        super::super::wav::Playback::open(&path, 49)?;
        Ok(())
    }
    fn inspect(&mut self) -> Result<bool> {
        let s = service::inspect_current(self.loaded)?;
        ensure!(
            matches!(s.active.as_str(), "active" | "inactive" | "failed"),
            "Service is transitioning; retry later"
        );
        if s.active == "active" {
            ensure!(s.pid != 0, "Service has no live process");
            return Ok(true);
        }
        ensure!(s.pid == 0, "Service still has a live process");
        drop(runtime::InstanceLock::acquire()?);
        Ok(false)
    }
    fn stop(&mut self) -> Result<()> {
        service::control(self.loaded, "stop")
    }
    fn restore(&mut self) -> Result<()> {
        service::control(self.loaded, "start")
    }
    fn play(&mut self, cancelled: &mut dyn FnMut() -> Result<bool>) -> Result<()> {
        self.validate()?;
        ensure!(
            !self.inspect()?,
            "Automatic playback started concurrently; test skipped"
        );
        let s = service::inspect_current(self.loaded)?;
        let _installed = service::require_installed_executable()?;
        let unit = format!(
            "piair-recording-{}-{}.service",
            s.user,
            uuid::Uuid::new_v4()
        );
        let args = arguments(&s, &unit, self.loaded, self.saved)?;
        ensure!(!cancelled()?, Cancelled);
        let mut child = Command::new("systemd-run")
            .args(&args)
            .stdin(Stdio::inherit())
            .spawn()
            .context("systemd-run unavailable; reinstall systemd")?;
        let mut authorized = false;
        let result = wait(&mut child, Duration::from_secs(140), &mut || {
            // Leave terminal input to systemd's normal polkit agent while it
            // authorizes creation; never race it for password input. SIGINT is
            // still observed. The recording service itself has null stdin.
            if !authorized {
                if self.cancel.load(Ordering::Relaxed) {
                    return Ok(true);
                }
                authorized = unit_state(&unit)?.lines().any(|l| l == "LoadState=loaded");
                if !authorized {
                    return Ok(false);
                }
            }
            cancelled()
        });
        // Stop the exact random unit we own, even after authorization failure or
        // cancellation. Never stop a wildcard, another user or a lock owner.
        let cleanup = settle_and_stop(&mut child, &unit);
        let _ = child.kill();
        let _ = child.wait();
        cleanup.context(Teardown)?;
        result.with_context(|| {
            format!("Recording service {unit} failed; inspect journalctl --system -u {unit}")
        })
    }
}
fn arguments(
    s: &service::Service,
    unit: &str,
    loaded: &Loaded,
    saved: &str,
) -> Result<Vec<String>> {
    // The transient service runs from the account home, not the wizard's cwd.
    // Resolve a supported relative --config path before crossing that boundary.
    let path = std::fs::canonicalize(&loaded.path)?;
    let mut args = vec![
        "--system".into(),
        "--wait".into(),
        "--collect".into(),
        format!("--unit={unit}"),
        format!("--uid={}", s.user),
        format!("--working-directory={}", s.home.display()),
    ];
    // Take sandbox properties from the compiled stock template, never from a
    // user-controlled unit or environment. No overrides or ExecStart mutation.
    let section = include_str!("../../../systemd/piair@.service")
        .split("[Service]\n")
        .nth(1)
        .unwrap()
        .split("[Install]")
        .next()
        .unwrap();
    for line in section.lines() {
        if let Some((key, _)) = line.split_once('=') {
            if !line.starts_with('#')
                && !matches!(
                    key,
                    "Type"
                        | "User"
                        | "WorkingDirectory"
                        | "ExecStart"
                        | "ExecStartPre"
                        | "Restart"
                        | "RestartSec"
                        | "StandardInput"
                        | "StandardOutput"
                        | "StandardError"
                )
            {
                args.push(format!("--property={line}"));
            }
        }
    }
    args.extend([
        "--property=Restart=no".into(),
        "--property=StandardInput=null".into(),
        "--property=RuntimeMaxSec=100s".into(),
        "--property=StandardOutput=journal".into(),
        "--property=StandardError=journal".into(),
        "/usr/local/bin/piair".into(),
        "--config".into(),
        path.to_string_lossy().into_owned(),
        "service-recording".into(),
        "--saved".into(),
        saved.into(),
    ]);
    Ok(args)
}
fn wait(
    child: &mut Child,
    limit: Duration,
    cancelled: &mut dyn FnMut() -> Result<bool>,
) -> Result<()> {
    let start = Instant::now();
    loop {
        ensure!(!cancelled()?, Cancelled);
        if let Some(status) = child.try_wait()? {
            ensure!(status.code() != Some(75), Teardown);
            ensure!(status.success(), "Temporary recording service failed or authorization was denied. Inspect journalctl --system -u 'piair-recording-*' for details");
            return Ok(());
        }
        ensure!(start.elapsed() < limit, "Recording service timed out");
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn unit_state(unit: &str) -> Result<String> {
    use std::io::Read;
    let mut child = Command::new("systemctl")
        .args([
            "--system",
            "--no-ask-password",
            "show",
            unit,
            "-p",
            "ActiveState",
            "-p",
            "LoadState",
            "-p",
            "MainPID",
            "-p",
            "ControlPID",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let result = wait(&mut child, Duration::from_secs(5), &mut || Ok(false));
    if child.try_wait()?.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        return result.map(|()| String::new());
    }
    let mut text = String::new();
    child
        .stdout
        .take()
        .context("missing systemctl output")?
        .read_to_string(&mut text)?;
    Ok(text)
}
fn settle_and_stop(client: &mut Child, unit: &str) -> Result<()> {
    // A cancelled systemd-run client can have a pending StartTransientUnit call.
    // Keep it alive until the manager acknowledges creation or the call returns,
    // so an early 'not found' cannot be mistaken for completed teardown.
    let start = Instant::now();
    while client.try_wait()?.is_none() && unit_state(unit)?.contains("LoadState=not-found") {
        ensure!(start.elapsed() < Duration::from_secs(10), "Test creation is still pending; inspect {unit}. No automatic restart. Manager lifetime is bounded to 100 seconds plus 30 seconds teardown");
        std::thread::sleep(Duration::from_millis(100));
    }
    stop_test(unit)
}
fn stop_test(unit: &str) -> Result<()> {
    let mut child = Command::new("systemctl")
        .args(["--system", "--no-ask-password", "stop", unit])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let result = wait(&mut child, Duration::from_secs(35), &mut || Ok(false));
    let _ = child.kill();
    let _ = child.wait();
    // --collect may have unloaded a completed unit. Absence is a safe result.
    let state = unit_state(unit)?;
    ensure!(stopped_state(&state),
        "Test teardown could not be verified. Inspect {unit} and explicitly stop it. It is bounded to 100 seconds plus 30 seconds teardown; automatic playback was not restarted. {result:?}");
    Ok(())
}

fn stopped_state(state: &str) -> bool {
    state.lines().any(|l| l == "LoadState=not-found")
        || (state
            .lines()
            .any(|l| matches!(l, "ActiveState=inactive" | "ActiveState=failed"))
            && state.lines().any(|l| l == "MainPID=0")
            && state.lines().any(|l| l == "ControlPID=0"))
}

#[cfg(test)]
mod tests;
