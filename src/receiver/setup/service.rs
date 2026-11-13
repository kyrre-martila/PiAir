//! Conservative activation of the installed stock system template via polkit.
//! Custom units/drop-ins stay a documented manual operation.
use crate::config::Loaded;
use anyhow::{ensure, Context, Result};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug)]
pub(super) struct Service {
    pub unit: String,
    pub status: String,
    pub home: PathBuf,
    pub user: String,
    pub active: String,
    pub boot: String,
    pub pid: u32,
    pub result: String,
}
fn output(args: &[&str]) -> Result<String> {
    let result = Command::new("systemctl")
        .args(["--system", "--no-pager", "--no-ask-password"])
        .args(args)
        .output()
        .context("systemctl unavailable")?;
    ensure!(
        result.status.success(),
        "systemd query failed; see docs/SYSTEMD.md"
    );
    Ok(String::from_utf8(result.stdout)?)
}
fn properties(text: &str) -> BTreeMap<&str, &str> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .collect()
}
fn trusted_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(path)?;
    ensure!(
        m.is_file() && m.uid() == 0 && m.mode() & 0o022 == 0,
        "service files must be root-owned regular files, unwritable by other users"
    );
    Ok(())
}
pub(super) fn inspect(loaded: &Loaded) -> Result<Service> {
    let service = inspect_current(loaded)?;
    ensure!(
        matches!(service.active.as_str(), "inactive" | "failed"),
        "service is already active or transitioning; stop it before setup"
    );
    Ok(service)
}
pub(super) fn inspect_current(loaded: &Loaded) -> Result<Service> {
    query(loaded, true)
}
pub(super) fn observe(loaded: &Loaded) -> Result<Service> {
    query(loaded, false)
}
fn query(loaded: &Loaded, verify: bool) -> Result<Service> {
    ensure!(
        Path::new("/run/systemd/system").is_dir(),
        "systemd is optional and is not running on this host"
    );
    let uid = rustix::process::geteuid().as_raw();
    ensure!(
        uid != 0,
        "activation requires the intended non-root account; run setup as that account"
    );
    let passwd = Command::new("getent")
        .args(["passwd", &uid.to_string()])
        .output()?;
    ensure!(
        passwd.status.success(),
        "cannot resolve current account through NSS"
    );
    let passwd = String::from_utf8(passwd.stdout)?;
    let rows: Vec<_> = passwd.lines().collect();
    ensure!(rows.len() == 1, "ambiguous current account");
    let fields: Vec<_> = rows[0].split(':').collect();
    ensure!(
        fields.len() == 7 && fields[2].parse::<u32>()? == uid,
        "invalid current account"
    );
    let user = fields[0];
    ensure!(
        !user.is_empty()
            && user
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            && !user.starts_with('-'),
        "account requires manual systemd escaping; see docs/SYSTEMD.md"
    );
    let unit = format!("piair@{user}.service");
    let text = output(&[
        "show",
        &unit,
        "-p",
        "LoadState",
        "-p",
        "MainPID",
        "-p",
        "Result",
        "-p",
        "ActiveState",
        "-p",
        "UnitFileState",
        "-p",
        "User",
        "-p",
        "Environment",
        "-p",
        "EnvironmentFiles",
        "-p",
        "DropInPaths",
        "-p",
        "FragmentPath",
        "-p",
        "ExecStart",
        "-p",
        "WorkingDirectory",
    ])?;
    let p = properties(&text);
    ensure!(p.get("LoadState") == Some(&"loaded"), "PiAir systemd template is not installed; installer is planned for PR #74; manual procedure: docs/SYSTEMD.md");
    ensure!(
        p.get("User") == Some(&user),
        "service account differs from current user"
    );
    let home = PathBuf::from(fields[5]);
    if verify {
        validate_properties(&p, user)?;
        let fragment = Path::new(p.get("FragmentPath").context("missing unit path")?);
        trusted_file(fragment)?;
        ensure!(
            std::fs::read_to_string(fragment)? == include_str!("../../../systemd/piair@.service"),
            "custom service template requires manual review; see docs/SYSTEMD.md"
        );
        trusted_file(Path::new("/usr/local/bin/piair"))?;
        validate_config_path(loaded, &home)?;
    }
    Ok(Service {
        unit,
        status: format!(
            "{}, {}",
            p.get("ActiveState").unwrap_or(&"unknown"),
            p.get("UnitFileState").unwrap_or(&"unknown")
        ),
        home,
        user: user.into(),
        active: p
            .get("ActiveState")
            .context("missing service state")?
            .to_string(),
        boot: p.get("UnitFileState").unwrap_or(&"unknown").to_string(),
        pid: p.get("MainPID").context("missing service PID")?.parse()?,
        result: p.get("Result").unwrap_or(&"unknown").to_string(),
    })
}
fn validate_properties(p: &BTreeMap<&str, &str>, user: &str) -> Result<()> {
    ensure!(p.get("LoadState")==Some(&"loaded"), "PiAir systemd template is not installed; installer is planned for PR #74; manual procedure: docs/SYSTEMD.md");
    ensure!(
        p.get("User") == Some(&user),
        "service account differs from current user"
    );
    for key in ["Environment", "EnvironmentFiles", "DropInPaths"] {
        ensure!(
            p.get(key) == Some(&""),
            "custom service environment/drop-ins require manual review: docs/SYSTEMD.md"
        );
    }
    ensure!(
        p.get("ExecStart").is_some_and(|v| v.starts_with(
            "{ path=/usr/local/bin/piair ; argv[]=/usr/local/bin/piair run --allow-unattended ;"
        ) && v.matches("argv[]=").count() == 1),
        "unexpected service executable or arguments"
    );
    ensure!(
        p.get("WorkingDirectory") == Some(&"~"),
        "unexpected service working directory"
    );
    ensure!(p.contains_key("UnitFileState"), "missing unit state");
    Ok(())
}
fn validate_config_path(loaded: &Loaded, home: &Path) -> Result<()> {
    ensure!(home.is_absolute(), "account home must be absolute");
    let saved = std::fs::canonicalize(&loaded.path)?;
    let expected = std::fs::canonicalize(home.join(".config/piair/config.toml"))?;
    ensure!(saved==expected, "service resolves HOME/.config/piair/config.toml, which differs from saved config; configure a reviewed drop-in manually (docs/SYSTEMD.md)");
    Ok(())
}
pub(super) fn activate(loaded: &Loaded, expected: &Service) -> Result<()> {
    let current = inspect(loaded)?;
    ensure!(
        current.unit == expected.unit
            && current.user == expected.user
            && current.home == expected.home,
        "service identity changed; retry setup"
    );
    super::super::runtime::Runtime::prepare(loaded, loaded.config.playback.effective()?)?;
    // Release immediately before systemd starts the runtime, which acquires the same lock.
    drop(super::super::runtime::InstanceLock::acquire()?);
    let status = Command::new("systemctl")
        .args(["--system", "enable", "--now", &current.unit])
        .status()
        .context("systemctl activation failed")?;
    ensure!(status.success(), "service activation was not completed; authorization may have been denied. Check systemctl status and docs/SYSTEMD.md; enablement may already be saved");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_unit_wrong_identity_and_overrides_fail_closed() {
        let text="LoadState=loaded\nUser=pi\nActiveState=inactive\nUnitFileState=disabled\nEnvironment=\nEnvironmentFiles=\nDropInPaths=\nWorkingDirectory=~\nExecStart={ path=/usr/local/bin/piair ; argv[]=/usr/local/bin/piair run --allow-unattended ; }";
        validate_properties(&properties(text), "pi").unwrap();
        for (from, to) in [
            ("LoadState=loaded", "LoadState=not-found"),
            ("User=pi", "User=root"),
            ("path=/usr/local/bin/piair ;", "path=/unexpected/program ;"),
            (
                "--allow-unattended ; }",
                "--allow-unattended ; } { path=/unexpected ; argv[]=unexpected ; }",
            ),
            ("Environment=\n", "Environment=PIAIR_CONFIG=/other\n"),
            ("DropInPaths=\n", "DropInPaths=/override\n"),
            (
                "--allow-unattended ;",
                "--allow-unattended --config other ;",
            ),
        ] {
            assert!(validate_properties(&properties(&text.replace(from, to)), "pi").is_err());
        }
    }
    #[test]
    fn config_path_must_match_account_home() {
        let d = tempfile::tempdir().unwrap();
        let default = d.path().join(".config/piair");
        std::fs::create_dir_all(&default).unwrap();
        let path = default.join("config.toml");
        std::fs::write(&path, "").unwrap();
        let loaded = crate::config::load(Some(path)).unwrap();
        validate_config_path(&loaded, d.path()).unwrap();
        assert!(validate_config_path(&loaded, Path::new("/other-account")).is_err());
    }
}

/// Normal authorization is left to systemctl/polkit. Never change enablement.
trait Backend {
    fn inspect(&mut self) -> Result<Service>;
    fn prepare(&mut self) -> Result<()>;
    fn lock_free(&mut self) -> Result<()>;
    fn invoke(&mut self, action: &str, unit: &str) -> Result<()>;
}
struct System<'a>(&'a Loaded);
impl Backend for System<'_> {
    fn inspect(&mut self) -> Result<Service> {
        inspect_current(self.0)
    }
    fn prepare(&mut self) -> Result<()> {
        super::runtime::Runtime::prepare(self.0, self.0.config.playback.effective()?)?;
        Ok(())
    }
    fn lock_free(&mut self) -> Result<()> {
        drop(super::runtime::InstanceLock::acquire()?);
        Ok(())
    }
    fn invoke(&mut self, action: &str, unit: &str) -> Result<()> {
        let status = Command::new("systemctl")
            .args(["--system", action, unit])
            .status()
            .context("cannot run systemctl; see docs/SYSTEMD.md")?;
        ensure!(
            status.success(),
            "service operation failed or authorization denied; see docs/SYSTEMD.md"
        );
        Ok(())
    }
}
pub(super) fn control(loaded: &Loaded, action: &str) -> Result<()> {
    control_with(&mut System(loaded), action)
}
fn control_with(backend: &mut impl Backend, action: &str) -> Result<()> {
    ensure!(
        matches!(action, "start" | "stop" | "restart"),
        "invalid service action"
    );
    let service = backend.inspect()?;
    let running = service.active == "active" && service.pid != 0;
    let stopped = matches!(service.active.as_str(), "inactive" | "failed") && service.pid == 0;
    ensure!(
        running || stopped,
        "service is transitioning; wait and retry"
    );
    if action == "start" && running {
        return Ok(());
    }
    if action == "stop" && stopped {
        return backend.lock_free();
    }
    if action != "stop" {
        backend.prepare()?;
        if stopped {
            backend.lock_free()?;
        }
    }
    backend.invoke(action, &service.unit)?;
    let after = backend.inspect()?;
    ensure!(
        after.unit == service.unit && after.user == service.user && after.home == service.home,
        "service identity changed; inspect it manually before continuing"
    );
    if action == "stop" {
        ensure!(
            matches!(after.active.as_str(), "inactive" | "failed") && after.pid == 0,
            "service has not stopped; no settings changed"
        );
        backend.lock_free()?;
    } else {
        ensure!(
            after.active == "active" && after.pid != 0,
            "service did not remain running; inspect piair logs"
        );
    }
    Ok(())
}
pub(super) fn logs(
    loaded: &Loaded,
    follow: bool,
    detailed: bool,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<()> {
    let service = inspect_current(loaded)?;
    println!(
        "Recent service history for {} (not current playback state):",
        service.unit
    );
    println!("If no entries appear, check journal permissions. Use piair -v logs for details.");
    let mut command = Command::new("journalctl");
    command.args(log_arguments(&service.unit, follow));
    stream_logs(&mut command, detailed, cancel)
}
fn stream_logs(
    command: &mut Command,
    detailed: bool,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<()> {
    use rustix::event::{poll, PollFd, PollFlags, Timespec};
    use std::io::{Read, Write};
    use std::sync::atomic::Ordering;
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("journalctl unavailable; see docs/SYSTEMD.md")?;
    let mut pipe = child.stdout.take().context("missing journal output")?;
    // Every exit, including broken output and cancellation, reaps journalctl.
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Child(child);
    let mut pending = Vec::new();
    let mut out = std::io::stdout();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut fds = [PollFd::new(&pipe, PollFlags::IN)];
        match poll(
            &mut fds,
            Some(&Timespec {
                tv_sec: 0,
                tv_nsec: 100_000_000,
            }),
        ) {
            Ok(0) | Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Err(e.into()),
            _ => {}
        }
        let mut bytes = [0u8; 8192];
        let n = match pipe.read(&mut bytes) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if n == 0 {
            break;
        }
        for byte in &bytes[..n] {
            if *byte == b'\n' {
                writeln!(
                    out,
                    "{}",
                    render_log(&String::from_utf8_lossy(&pending), detailed)
                )?;
                out.flush()?;
                pending.clear();
            } else {
                ensure!(
                    pending.len() < 1024 * 1024,
                    "journal entry too large; use journalctl directly for diagnostics"
                );
                pending.push(*byte);
            }
        }
    }
    if !pending.is_empty() {
        writeln!(
            out,
            "{}",
            render_log(&String::from_utf8_lossy(&pending), detailed)
        )?;
    }
    let status = child.0.wait()?;
    ensure!(
        status.success() || cancel.load(Ordering::Relaxed),
        "cannot read service logs; check journal permissions (docs/SYSTEMD.md)"
    );
    Ok(())
}
fn log_arguments(unit: &str, follow: bool) -> Vec<&str> {
    let mut args = vec![
        "--system",
        "--no-pager",
        "--unit",
        unit,
        "--lines=40",
        "--output=json",
    ];
    if follow {
        args.push("--follow");
    }
    args
}
// Coreutils date uses the host's local timezone, including DST, without unsafe
// localtime calls or another time dependency. Only validated epoch seconds are
// passed as an argument; unavailable/invalid times never leak raw microseconds.
fn journal_local_time(micros: Option<&str>, command: &mut Command) -> String {
    let Some(seconds) = micros
        .and_then(|v| v.parse::<u64>().ok())
        .map(|v| v / 1_000_000)
    else {
        return "Time unavailable".into();
    };
    command
        .args(["--date", &format!("@{seconds}"), "+%Y-%m-%d %H:%M:%S %z"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|text| super::clean(text.trim()))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "Time unavailable".into())
}

fn render_log(line: &str, detailed: bool) -> String {
    use serde_json::Value;
    let Ok(entry) = serde_json::from_str::<Value>(line) else {
        return super::clean(line);
    };
    let Some(message) = entry.get("MESSAGE").and_then(Value::as_str) else {
        return "Non-text journal entry; see docs/SYSTEMD.md".into();
    };
    let event = serde_json::from_str::<Value>(message).ok();
    let timestamp = event
        .as_ref()
        .and_then(|event| event.get("timestamp"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(super::clean)
        .unwrap_or_else(|| {
            journal_local_time(
                entry.get("__REALTIME_TIMESTAMP").and_then(Value::as_str),
                &mut Command::new("date"),
            )
        });
    let Some(event) = event else {
        return format!("{timestamp}: {}", super::clean(message));
    };
    let mut text = format!(
        "{} {}",
        timestamp,
        event.get("level").and_then(Value::as_str).unwrap_or("")
    );
    if let Some(fields) = event.get("fields").and_then(Value::as_object) {
        for (key, value) in fields {
            // Application logging deliberately excludes secret material. Keep a
            // second boundary here and expose only known diagnostic fields.
            let relevant = matches!(
                key.as_str(),
                "message" | "event" | "error" | "diagnostic" | "delay_secs" | "success"
            );
            let diagnostic = detailed
                && matches!(
                    key.as_str(),
                    "phase"
                        | "protocol"
                        | "status"
                        | "reason"
                        | "body_length"
                        | "receiver_id"
                        | "capture_device"
                        | "request"
                        | "service"
                );
            if relevant || diagnostic {
                text.push_str(&format!(
                    " {key}={}",
                    super::clean(value.as_str().unwrap_or(&value.to_string()))
                ));
            }
        }
    }
    text
}

#[cfg(test)]
mod daily_tests {
    use super::*;
    struct Mock {
        active: bool,
        missing: bool,
        denied: bool,
        locked: bool,
        calls: Vec<String>,
    }
    impl Backend for Mock {
        fn inspect(&mut self) -> Result<Service> {
            ensure!(
                !self.missing,
                "service not installed; PR #74; docs/SYSTEMD.md"
            );
            Ok(Service {
                unit: "piair@alice.service".into(),
                user: "alice".into(),
                home: "/users/alice".into(),
                status: "".into(),
                active: if self.active { "active" } else { "inactive" }.into(),
                pid: if self.active { 42 } else { 0 },
                boot: "disabled".into(),
                result: "success".into(),
            })
        }
        fn prepare(&mut self) -> Result<()> {
            self.calls.push("prepare".into());
            Ok(())
        }
        fn lock_free(&mut self) -> Result<()> {
            ensure!(!self.locked, "manual runtime owns lock");
            Ok(())
        }
        fn invoke(&mut self, action: &str, unit: &str) -> Result<()> {
            self.calls.push(format!("{action} {unit}"));
            ensure!(!self.denied, "authorization denied");
            self.active = action != "stop";
            Ok(())
        }
    }
    fn mock(active: bool) -> Mock {
        Mock {
            active,
            missing: false,
            denied: false,
            locked: false,
            calls: Vec::new(),
        }
    }
    #[test]
    fn controls_only_verified_account_without_changing_enablement() {
        for action in ["start", "stop", "restart"] {
            let mut m = mock(action != "start");
            control_with(&mut m, action).unwrap();
            assert!(m.calls.contains(&format!("{action} piair@alice.service")));
            assert!(!m
                .calls
                .iter()
                .any(|s| s.contains("enable") || s.contains("disable")));
            assert_eq!(m.inspect().unwrap().boot, "disabled");
        }
        for (active, action) in [(true, "start"), (false, "stop")] {
            let mut m = mock(active);
            control_with(&mut m, action).unwrap();
            assert!(m.calls.is_empty());
        }
    }
    #[test]
    fn missing_denied_or_manually_locked_services_fail_closed() {
        let mut m = mock(false);
        m.missing = true;
        assert!(control_with(&mut m, "start").is_err());
        assert!(m.calls.is_empty());
        let mut m = mock(false);
        m.denied = true;
        assert!(control_with(&mut m, "start")
            .unwrap_err()
            .to_string()
            .contains("authorization"));
        assert!(!m.active);
        let mut m = mock(false);
        m.locked = true;
        assert!(control_with(&mut m, "start").is_err());
        assert!(!m.active);
        assert!(!m.calls.iter().any(|s| s.starts_with("start ")));
        assert!(control_with(&mut m, "enable").is_err());
    }
}

#[cfg(test)]
mod log_tests {
    use super::*;
    #[test]
    fn recent_and_follow_logs_are_scoped_to_one_unit() {
        for follow in [false, true] {
            let args = log_arguments("piair@alice.service", follow);
            assert!(args
                .windows(2)
                .any(|w| w == ["--unit", "piair@alice.service"]));
            assert!(args.contains(&"--lines=40"));
            assert_eq!(args.contains(&"--follow"), follow);
        }
    }
    #[test]
    fn structured_logs_keep_errors_and_safe_details_without_secret_fields() {
        let message = serde_json::json!({"timestamp":"2026-11-13T12:00:00Z", "level":"ERROR", "fields":{
            "message":"session failed", "event":"retry", "error":"network unavailable", "phase":"authentication", "seed":"SECRET", "payload":"PRIVATE", "pin":"1234"
        }}).to_string();
        let entry =
            serde_json::json!({"MESSAGE":message, "__REALTIME_TIMESTAMP":"123456"}).to_string();
        for detailed in [false, true] {
            let text = render_log(&entry, detailed);
            assert!(text.contains("session failed"));
            assert!(text.contains("network unavailable"));
            assert!(!text.contains("SECRET"));
            assert!(!text.contains("PRIVATE"));
            assert!(!text.contains("1234"));
            assert_eq!(text.contains("phase=authentication"), detailed);
        }
        let entry = serde_json::json!({"MESSAGE":"Main process exited with status 1", "__REALTIME_TIMESTAMP":"42"}).to_string();
        assert!(render_log(&entry, false).contains("Main process exited"));
    }
    #[test]
    fn follow_cancellation_reaps_child_and_journal_errors_are_actionable() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            signal.store(true, Ordering::Relaxed);
        });
        let mut command = Command::new("python3");
        command.args(["-c", "import time; time.sleep(30)"]);
        let start = std::time::Instant::now();
        stream_logs(&mut command, false, &cancel).unwrap();
        worker.join().unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        let mut command = Command::new("python3");
        command.args(["-c", "raise SystemExit(3)"]);
        let error = stream_logs(&mut command, false, &AtomicBool::new(false)).unwrap_err();
        assert!(error.to_string().contains("journal permissions"));
    }
    #[test]
    fn journal_fallback_uses_readable_local_time_including_timezone_and_dst() {
        for (zone, expected) in [
            ("UTC", "2023-11-14 22:13:20 +0000"),
            ("America/New_York", "2023-11-14 17:13:20 -0500"),
        ] {
            let mut command = Command::new("date");
            command.env("TZ", zone);
            assert_eq!(
                journal_local_time(Some("1700000000123456"), &mut command),
                expected
            );
        }
        let mut summer = Command::new("date");
        summer.env("TZ", "America/New_York");
        assert_eq!(
            journal_local_time(Some("1689379200000000"), &mut summer),
            "2023-07-14 20:00:00 -0400"
        );
        for timestamp in [None, Some("invalid"), Some("-1")] {
            assert_eq!(
                journal_local_time(timestamp, &mut Command::new("date")),
                "Time unavailable"
            );
        }
        let entry = serde_json::json!({"MESSAGE":"Service stopped", "__REALTIME_TIMESTAMP":"1700000000123456"}).to_string();
        let rendered = render_log(&entry, false);
        assert!(!rendered.contains("1700000000123456"));
        assert!(rendered.contains(":00") || rendered.contains(":20"));
        let entry = serde_json::json!({"MESSAGE":"{\"fields\":{\"message\":\"failed\"}}", "__REALTIME_TIMESTAMP":"1700000000123456"}).to_string();
        assert!(!render_log(&entry, false).contains("1700000000123456"));
    }
}
