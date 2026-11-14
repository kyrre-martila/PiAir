use super::*;
use std::io::BufRead;
#[derive(Default)]
struct Mock {
    running: bool,
    calls: Vec<&'static str>,
    fail: Option<&'static str>,
}
impl Mock {
    fn action(&mut self, name: &'static str) -> Result<()> {
        self.calls.push(name);
        ensure!(self.fail != Some(name), "mock {name} denied or failed");
        Ok(())
    }
}
impl Backend for Mock {
    fn inspect(&mut self) -> Result<bool> {
        self.action("inspect")?;
        Ok(self.running)
    }
    fn validate(&mut self) -> Result<()> {
        self.action("validate")
    }
    fn stop(&mut self) -> Result<()> {
        self.action("stop")?;
        self.running = false;
        Ok(())
    }
    fn play(&mut self, _: &mut dyn FnMut() -> Result<bool>) -> Result<()> {
        if self.fail == Some("teardown") {
            self.calls.push("play");
            return Err(Teardown.into());
        }
        if self.fail == Some("cancelled") {
            self.calls.push("play");
            return Err(Cancelled.into());
        }
        self.action("play")
    }
    fn restore(&mut self) -> Result<()> {
        self.action("restore")?;
        self.running = true;
        Ok(())
    }
}
fn exercise(mock: &mut Mock, answers: &str, cancel: bool) -> (Result<()>, String) {
    let mut input = answers.as_bytes();
    let mut ui = Ui {
        read: || {
            let mut line = String::new();
            input.read_line(&mut line)?;
            Ok(line)
        },
        out: Vec::new(),
    };
    let result = workflow(mock, &mut ui, &AtomicBool::new(cancel));
    (result.map(|_| ()), String::from_utf8(ui.out).unwrap())
}
#[test]
fn skip_q_eof_never_inspect_or_change_service() {
    for answer in ["\n", "2\n", "q\n", ""] {
        let mut mock = Mock {
            running: true,
            ..Mock::default()
        };
        let _ = exercise(&mut mock, answer, false);
        assert!(mock.calls.is_empty());
        assert!(mock.running);
    }
}
#[test]
fn stopped_enabled_or_disabled_never_enable_or_restart() {
    // Boot state is deliberately absent from this protocol: neither branch
    // can mutate it. Both enabled and disabled use exactly the same path.
    for _enabled in [true, false] {
        let mut mock = Mock::default();
        let (result, text) = exercise(&mut mock, "1\n1\n", false);
        result.unwrap();
        assert_eq!(mock.calls, ["validate", "inspect", "play"]);
        assert!(!mock.running);
        assert!(text.contains("confirmed audible output"));
    }
}
#[test]
fn running_interruption_and_restore_are_separate_explicit_choices() {
    for (answers, calls, running) in [
        ("1\n2\n", vec!["validate", "inspect"], true),
        (
            "1\n1\n1\n2\n",
            vec!["validate", "inspect", "stop", "play"],
            false,
        ),
        (
            "1\n1\n1\n1\n",
            vec!["validate", "inspect", "stop", "play", "restore"],
            true,
        ),
    ] {
        let mut mock = Mock {
            running: true,
            ..Mock::default()
        };
        exercise(&mut mock, answers, false).0.unwrap();
        assert_eq!(mock.calls, calls);
        assert_eq!(mock.running, running);
    }
}
#[test]
fn unavailable_custom_permission_conflict_and_playback_failure_retain_setup() {
    for fail in ["validate", "inspect", "stop", "play", "restore"] {
        let mut mock = Mock {
            running: true,
            fail: Some(fail),
            ..Mock::default()
        };
        let (result, text) = exercise(&mut mock, "1\n1\n1\n1\n", false);
        result.unwrap();
        assert!(
            text.contains("unavailable") || text.contains("Could not") || text.contains("failed")
        );
        if matches!(fail, "validate" | "inspect" | "stop") {
            assert!(!mock.calls.contains(&"play"));
        }
    }
}
#[test]
fn negative_listening_answer_does_not_invalidate_settings() {
    let mut mock = Mock::default();
    let (result, text) = exercise(&mut mock, "1\n2\n", false);
    result.unwrap();
    assert!(text.contains("Check the speaker volume"));
    assert!(!text.contains("confirmed audible output"));
}
#[test]
fn cancelled_after_interruption_never_restores() {
    for answers in ["1\n1\nq\n", "1\n1\n", "1\n1\n1\nq\n"] {
        let mut mock = Mock {
            running: true,
            ..Mock::default()
        };
        assert!(exercise(&mut mock, answers, false).0.is_err());
        assert!(!mock.running);
        assert!(!mock.calls.contains(&"restore"));
    }
    let mut mock = Mock {
        running: true,
        ..Mock::default()
    };
    assert!(exercise(&mut mock, "1\n1\n", true).0.is_err());
    assert!(!mock.calls.contains(&"restore"));
}
#[test]
fn saved_fingerprint_rejects_edit_replacement_and_stale_loaded_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "# preserved comment\n").unwrap();
    let loaded = config::load(Some(path.clone())).unwrap();
    let saved = fingerprint(&path).unwrap();
    check_saved(&loaded, &saved).unwrap();
    std::fs::write(&path, "# changed\n").unwrap();
    assert!(check_saved(&loaded, &saved).is_err());
    let saved = fingerprint(&path).unwrap();
    std::fs::write(&path, "# changed\n").unwrap();
    assert!(check_saved(&loaded, &saved).is_err());
    std::fs::write(&path, "discovery_timeout_secs = 3\n").unwrap();
    assert!(check_saved(&loaded, &fingerprint(&path).unwrap()).is_err());
}
#[test]
fn transient_contract_is_fixed_nonroot_bounded_and_matches_stock_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "").unwrap();
    let loaded = config::load(Some(path)).unwrap();
    let service = service::Service {
        unit: "piair@pi.service".into(),
        status: "inactive".into(),
        home: "/home/pi".into(),
        user: "pi".into(),
        active: "inactive".into(),
        boot: "disabled".into(),
        pid: 0,
        result: "success".into(),
    };
    let args = arguments(
        &service,
        "piair-recording-pi-unique.service",
        &loaded,
        "hash",
    );
    for expected in [
        "--uid=pi",
        "--collect",
        "--wait",
        "--property=RuntimeMaxSec=100s",
        "--property=Restart=no",
        "--property=AmbientCapabilities=CAP_NET_BIND_SERVICE",
        "--property=CapabilityBoundingSet=CAP_NET_BIND_SERVICE",
        "--property=NoNewPrivileges=yes",
        "--property=KillMode=control-group",
        "--property=PrivateTmp=yes",
    ] {
        assert!(args.iter().any(|s| s == expected), "{expected}");
    }
    assert!(args.ends_with(&["service-recording".into(), "--saved".into(), "hash".into()]));
    assert!(!args
        .iter()
        .any(|s| s.contains("enable") || s.contains("volume") || s.contains("ExecStart=")));
}
#[test]
fn child_wait_is_bounded_and_cancelled_without_hardware() {
    for cancel in [false, true] {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        assert!(wait(&mut child, Duration::from_millis(10), &mut || Ok(cancel)).is_err());
        child.kill().unwrap();
        child.wait().unwrap();
    }
    let mut child = Command::new("true").spawn().unwrap();
    wait(&mut child, Duration::from_secs(1), &mut || Ok(false)).unwrap();
}
#[test]
fn wizard_order_saves_then_boot_then_recording_then_complete() {
    let source = include_str!("../mod.rs");
    let flow = source
        .split("pub(crate) fn run(")
        .nth(1)
        .unwrap()
        .split("fn requirements")
        .next()
        .unwrap();
    let mut last = 0;
    for step in [
        "select_input(",
        "choose_receiver(",
        "authentication(",
        "advanced_settings(",
        "save_playback_checked(",
        "optional_service(",
        "recording::offer(",
        "Setup complete.",
    ] {
        let position = flow.find(step).unwrap();
        assert!(position > last, "{step}");
        last = position;
    }
    assert!(!flow.contains("test_audio("));
}
#[test]
fn teardown_uncertainty_and_playback_cancellation_never_restore() {
    for fail in ["teardown", "cancelled"] {
        let mut mock = Mock {
            running: true,
            fail: Some(fail),
            ..Mock::default()
        };
        let (result, text) = exercise(&mut mock, "1\n1\n1\n", false);
        assert!(!mock.calls.contains(&"restore"));
        assert!(!mock.running);
        if fail == "teardown" {
            result.unwrap();
            assert!(text.contains("will not be restarted"));
        } else {
            assert!(result.unwrap_err().is::<Cancelled>());
        }
    }
    for state in [
        "LoadState=not-found\n",
        "ActiveState=inactive\nMainPID=0\nControlPID=0\n",
        "ActiveState=failed\nMainPID=0\nControlPID=0\n",
    ] {
        assert!(stopped_state(state));
    }
    for state in [
        "",
        "ActiveState=active\nMainPID=1",
        "ActiveState=failed\nMainPID=1\nControlPID=0",
        "ActiveState=deactivating\nMainPID=0\nControlPID=0",
        "ActiveState=inactive\n",
    ] {
        assert!(!stopped_state(state));
    }
    let mut child = Command::new("sh").args(["-c", "exit 75"]).spawn().unwrap();
    assert!(wait(&mut child, Duration::from_secs(1), &mut || Ok(false))
        .unwrap_err()
        .is::<Teardown>());
}
#[test]
fn existing_recording_uses_saved_receiver_authentication_and_preserves_files_and_volume() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let credentials = dir.path().join("pairing-fixture");
    std::fs::write(&credentials, b"unchanged credential bytes").unwrap();
    let wav = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/assets/audio/Pachelbel_Canon_audio_test.wav");
    for (method, transient) in [("stored-hap", false), ("own-tone-transient", true)] {
        let text = format!("# keep comments\n[playback]\nreceiver_id='device:aabbccddeeff'\nauthentication='{method}'\nvolume_db=-25\n");
        std::fs::write(&path, &text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let loaded = config::load(Some(path.clone())).unwrap();
        recording_operation(&loaded, &wav, |id, auth, wav, volume| {
            assert_eq!(id, "device:aabbccddeeff");
            assert_eq!(auth, transient);
            assert_eq!(volume, None);
            assert_eq!(loaded.config.playback.volume_db, Some(-25));
            assert!(!wav.frames.is_empty());
            assert!(wav.frames.len() <= 49 * 44100);
            Ok(())
        })
        .unwrap();
        assert!(recording_operation(
            &loaded,
            &dir.path().join("missing.wav"),
            |_, _, _, _| panic!("missing recording must not send")
        )
        .is_err());
        assert!(
            recording_operation(&loaded, &wav, |_, _, _, _| anyhow::bail!(
                "playback fixture failed"
            ))
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o640);
        assert_eq!(
            std::fs::read(&credentials).unwrap(),
            b"unchanged credential bytes"
        );
    }
}
#[test]
fn recording_q_eof_and_sigint_over_a_real_pty_clean_up_without_restart() {
    use std::{
        fs::File,
        io::{Read, Write},
        os::unix::ffi::OsStrExt,
    };
    const FIXTURE: &str = "PIAIR_RECORDING_PTY_FIXTURE";
    if let Some(marker) = std::env::var_os(FIXTURE) {
        struct Fixture(std::path::PathBuf);
        impl Backend for Fixture {
            fn validate(&mut self) -> Result<()> {
                Ok(())
            }
            fn inspect(&mut self) -> Result<bool> {
                Ok(true)
            }
            fn stop(&mut self) -> Result<()> {
                Ok(())
            }
            fn restore(&mut self) -> Result<()> {
                panic!("cancellation must not restart")
            }
            fn play(&mut self, cancelled: &mut dyn FnMut() -> Result<bool>) -> Result<()> {
                let mut child = Command::new("sleep").arg("30").spawn()?;
                println!("FIXTURE_PLAYING");
                let result = wait(&mut child, Duration::from_secs(5), cancelled);
                child.kill()?;
                child.wait()?;
                std::fs::write(&self.0, "cleaned")?;
                result
            }
        }
        let cancel = runtime::signal_cancellation().unwrap();
        let mut ui = Ui {
            read: || ui::terminal_line(&cancel),
            out: io::stdout(),
        };
        let result = workflow(&mut Fixture(marker.into()), &mut ui, &cancel);
        assert!(result.unwrap_err().is::<Cancelled>());
        return;
    }
    use rustix::pty::*;
    for mode in ["q", "eof", "sigint"] {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("cleanup");
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        rustix::fs::fcntl_setfl(&master, rustix::fs::OFlags::NONBLOCK).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let name = ptsname(&master, Vec::new()).unwrap();
        let slave = File::options()
            .read(true)
            .write(true)
            .open(std::ffi::OsStr::from_bytes(name.to_bytes()))
            .unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "receiver::setup::recording::tests::recording_q_eof_and_sigint_over_a_real_pty_clean_up_without_restart", "--nocapture"])
            .env(FIXTURE, &marker)
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave)).spawn().unwrap();
        let mut master = File::from(master);
        let result = (|| -> Result<()> {
            master.write_all(b"1\n1\n")?;
            let mut output = String::new();
            let start = Instant::now();
            while !output.contains("FIXTURE_PLAYING") {
                let mut buf = [0; 4096];
                match master.read(&mut buf) {
                    Ok(n) => output.push_str(&String::from_utf8_lossy(&buf[..n])),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                    Err(e) => return Err(e.into()),
                }
                ensure!(
                    start.elapsed() < Duration::from_secs(8),
                    "PTY fixture did not start: {output}"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            match mode {
                "q" => master.write_all(b"q\n")?,
                "eof" => master.write_all(b"\x04")?,
                _ => {
                    ensure!(
                        Command::new("kill")
                            .args(["-INT", &child.id().to_string()])
                            .status()?
                            .success(),
                        "SIGINT failed"
                    );
                }
            }
            while child.try_wait()?.is_none() {
                ensure!(
                    start.elapsed() < Duration::from_secs(10),
                    "PTY fixture failed to cancel"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            ensure!(child.wait()?.success(), "PTY fixture failed: {output}");
            ensure!(
                std::fs::read_to_string(&marker)? == "cleaned",
                "test resources not released"
            );
            Ok(())
        })();
        let _ = child.kill();
        let _ = child.wait();
        result.unwrap();
    }
}
