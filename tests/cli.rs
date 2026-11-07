use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_piair"))
        .args(args)
        .env_remove("PIAIR_CONFIG")
        .env_remove("XDG_CONFIG_HOME")
        .env("HOME", "/nonexistent/piair-cli-test")
        .output()
        .unwrap()
}

#[test]
fn help_and_version_ignore_missing_explicit_config() {
    for option in ["--help", "--version"] {
        let output = run(&["--config", "/nonexistent/piair.toml", option]);
        assert!(output.status.success());
        assert!(String::from_utf8(output.stdout).unwrap().contains("piair"));
    }
}

#[test]
fn local_commands_use_defaults_without_writing() {
    let output = run(&["status"]);
    assert!(output.status.success());
    let status = String::from_utf8(output.stdout).unwrap();
    assert!(status.contains("built-in defaults; file absent"));
    assert!(status.contains("AirPlay playback: not implemented"));
    assert!(run(&["config", "check"]).status.success());
    let output = run(&["config", "show"]);
    assert!(output.status.success());
    let shown = String::from_utf8(output.stdout).unwrap();
    assert!(shown.contains("discovery_timeout_secs = 5"));
    assert!(shown.contains("receiver_allowlist = []"));
}

#[test]
fn explicit_missing_config_is_an_actionable_failure() {
    let output = run(&["--config", "/nonexistent/piair.toml", "config", "check"]);
    assert_eq!(output.status.code(), Some(1));
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("cannot read /nonexistent/piair.toml"));
    assert!(error.contains("check path and read permissions"));
}

#[test]
fn audio_help_and_duration_validation_are_device_free() {
    for args in [
        vec!["audio", "--help"],
        vec!["audio", "monitor", "--help"],
        vec!["audio", "calibrate", "--help"],
    ] {
        assert!(run(&args).status.success());
    }
    for args in [
        vec!["audio", "monitor", "--duration-secs", "0"],
        vec!["audio", "monitor", "--duration-secs", "86401"],
        vec!["audio", "calibrate", "--duration-secs", "4"],
        vec!["audio", "calibrate", "--duration-secs", "301"],
    ] {
        assert_eq!(run(&args).status.code(), Some(2));
    }
}

#[test]
fn local_commands_and_declined_calibration_never_open_input() {
    use std::{fs, io::Write, process::Stdio};
    let dir = std::env::temp_dir().join(format!("piair-cli-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let path = dir.join("config.toml");
    let source = "[audio]\ndevice = 'piair-nonexistent-device-do-not-open'\n";
    fs::write(&path, source).unwrap();
    for args in [
        vec!["status"],
        vec!["config", "check"],
        vec!["config", "show"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_piair"))
            .arg("--config")
            .arg(&path)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("ALSA"));
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_piair"))
        .arg("--config")
        .arg(&path)
        .args(["audio", "calibrate"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"no\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("ALSA"));
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn alsa_file_fixture_preserves_right_channel_and_sample_time_transitions() {
    use std::fs;
    let dir = std::env::temp_dir().join(format!("piair-pcm-fixture-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let raw = dir.join("input.raw");
    let mut pcm = Vec::new();
    for right in [0_i32, 1 << 30, 0] {
        for _ in 0..4800 {
            pcm.extend_from_slice(&0_i32.to_ne_bytes());
            pcm.extend_from_slice(&right.to_ne_bytes());
        }
    }
    fs::write(&raw, pcm).unwrap();
    let alsa = dir.join("alsa.conf");
    fs::write(&alsa, format!(
        "pcm.fixture {{ type file slave.pcm {{ type null }} infile \"{}\" file \"/dev/null\" format \"raw\" }}\n", raw.display()
    )).unwrap();
    let config = dir.join("config.toml");
    fs::write(
        &config,
        "[audio]\ndevice='fixture'\nstart_duration_ms=40\nstop_duration_ms=60\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_piair"))
        .arg("--config")
        .arg(&config)
        .args(["audio", "monitor", "--duration-secs", "1"])
        .env("ALSA_CONFIG_PATH", &alsa)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("L=-inf R=-6.0 combined=-6.0"));
    let states: Vec<_> = text
        .lines()
        .filter(|line| line.contains("state="))
        .take(5)
        .collect();
    for (line, (frame, state)) in states.iter().zip([
        (960, "IDLE"),
        (5760, "STARTING"),
        (6720, "STREAMING"),
        (10560, "STOPPING"),
        (12480, "IDLE"),
    ]) {
        assert!(line.contains(&format!("frame={frame} ")), "{line}");
        assert!(line.contains(&format!("state={state} ")), "{line}");
    }
    assert_eq!(states.len(), 5);
    assert!(text.contains("event=Some(Started)"));
    assert!(text.contains("event=Some(Stopped)"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn receiver_identity_operations_fail_closed_without_interaction_or_discovery() {
    for command in ["pair", "authenticate", "select"] {
        for id in [
            "Living Room",
            "192.0.2.1",
            "room.local",
            "device:AABBCCDDEEFF",
            "device:000000000000",
            "device:aabbccddeeff",
        ] {
            let output = run(&[command, id]);
            assert_eq!(output.status.code(), Some(1));
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!stderr.contains("mDNS"));
            assert!(!stderr.contains("connection failed"));
        }
    }
}

#[test]
fn approved_pairing_refuses_piped_input_and_local_commands_do_not_connect() {
    use std::{fs, net::TcpListener};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "receiver_allowlist=['device:aabbccddeeff']\n").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    for args in [
        vec!["status"],
        vec!["config", "show"],
        vec!["config", "check"],
        vec!["unpair", "device:aabbccddeeff"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_piair"))
            .arg("--config")
            .arg(&path)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(listener.accept().is_err());
    }
    for command in ["pair", "authenticate"] {
        let output = Command::new(env!("CARGO_BIN_EXE_piair"))
            .arg("--config")
            .arg(&path)
            .args([command, "device:aabbccddeeff"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
    }
    assert!(!dir.path().join("receiver-state").exists());
}

#[test]
fn pairing_profile_is_explicit_and_never_auto_negotiated() {
    for method in ["pin-display", "setup-code", "transient"] {
        let output = run(&["select", "device:aabbccddeeff", "--method", method]);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("unapproved"));
    }
    assert_eq!(
        run(&["select", "device:aabbccddeeff", "--method", "auto"])
            .status
            .code(),
        Some(2)
    );
    assert!(String::from_utf8_lossy(&run(&["select", "--help"]).stdout).contains("setup-code"));
}
