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
    assert!(status.contains("activity unknown"));
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

#[test]
fn control_diagnostic_is_opt_in_and_cannot_bypass_authentication() {
    assert!(run(&["authenticate", "--help"]).status.success());
    let output = run(&["authenticate", "device:aabbccddeeff", "--control-check"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not approved"));
    assert_eq!(run(&["status", "--control-check"]).status.code(), Some(2));
    assert_eq!(
        run(&["pair", "device:aabbccddeeff", "--control-check"])
            .status
            .code(),
        Some(2)
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "receiver_allowlist=['device:aabbccddeeff']").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_piair"))
        .arg("--config")
        .arg(path)
        .args(["authenticate", "device:aabbccddeeff", "--control-check"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
    assert!(!dir.path().join("receiver-state").exists());
}

#[test]
fn realtime_diagnostic_requires_approval_interaction_and_bounded_duration() {
    for duration in ["0", "4"] {
        assert_eq!(
            run(&["stream-test", "device:aabbccddeeff", "--seconds", duration])
                .status
                .code(),
            Some(2)
        );
    }
    let unapproved = run(&["stream-test", "device:aabbccddeeff"]);
    assert!(!unapproved.status.success());
    assert!(String::from_utf8_lossy(&unapproved.stderr).contains("not approved"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "receiver_allowlist=['device:aabbccddeeff']").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_piair"))
        .arg("--config")
        .arg(path)
        .args(["stream-test", "device:aabbccddeeff"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
    assert!(!dir.path().join("receiver-state").exists());
}

#[test]
fn pyatv_preamble_option_is_authenticate_only_and_disabled_by_default() {
    let help = run(&["authenticate", "--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout)
        .unwrap()
        .contains("--experimental-pyatv-transient-preamble"));
    for command in ["pair", "stream-test", "select"] {
        let output = run(&[
            command,
            "device:aabbccddeeff",
            "--experimental-pyatv-transient-preamble",
        ]);
        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
fn owntone_experiment_is_authentication_or_explicit_setup_only_and_mutually_exclusive() {
    let flag = "--experimental-owntone-transient";
    let help = run(&["authenticate", "--help"]);
    assert!(String::from_utf8(help.stdout).unwrap().contains(flag));
    let help = run(&["stream-test", "--help"]);
    assert!(String::from_utf8(help.stdout).unwrap().contains(flag));
    let allowed = run(&["stream-test", "device:aabbccddeeff", "--setup-check", flag]);
    assert_eq!(allowed.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&allowed.stderr).contains("not approved"));
    let picker = run(&["stream-test", "--setup-check", flag]);
    assert_eq!(picker.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&picker.stderr).contains("interactive terminal"));
    for extra in [
        vec!["--setup-profile", "a"],
        vec!["--setup-profile", "b"],
        vec!["--setup-profile", "c"],
    ] {
        let mut args = vec!["stream-test", "--setup-check", flag];
        if !extra.is_empty() {
            args.insert(1, "device:aabbccddeeff");
        }
        args.extend(extra);
        assert_eq!(run(&args).status.code(), Some(2));
    }
    for command in ["pair", "stream-test", "select"] {
        assert_eq!(
            run(&[command, "device:aabbccddeeff", flag]).status.code(),
            Some(2)
        );
    }
    assert_eq!(
        run(&[
            "authenticate",
            "device:aabbccddeeff",
            flag,
            "--experimental-pyatv-transient-preamble"
        ])
        .status
        .code(),
        Some(2)
    );
    assert!(
        String::from_utf8_lossy(&run(&["authenticate", "device:aabbccddeeff", flag]).stderr)
            .contains("not approved")
    );
}

#[test]
fn setup_profiles_are_explicit_setup_only_and_keep_authorization() {
    for profile in ["a", "b", "c"] {
        assert_eq!(
            run(&[
                "stream-test",
                "device:aabbccddeeff",
                "--setup-profile",
                profile
            ])
            .status
            .code(),
            Some(2)
        );
        let output = run(&[
            "stream-test",
            "device:aabbccddeeff",
            "--setup-check",
            "--setup-profile",
            profile,
        ]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("not approved"));
    }
    assert_eq!(
        run(&[
            "stream-test",
            "device:aabbccddeeff",
            "--setup-check",
            "--setup-profile",
            "d"
        ])
        .status
        .code(),
        Some(2)
    );
}

#[test]
fn interactive_commands_fail_before_discovery_or_state_writes_without_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "receiver_allowlist=['device:aabbccddeeff']").unwrap();
    for args in [
        vec!["pairing"],
        vec!["stream-test"],
        vec!["stream-test", "--setup-check", "--volume-db", "-15"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_piair"))
            .arg("--config")
            .arg(&config)
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
        assert!(!dir.path().join("receiver-state").exists());
    }
}

#[test]
fn pairing_help_identifies_enrollment_prerequisites() {
    let help = run(&["--help"]);
    let text = String::from_utf8(help.stdout).unwrap();
    assert!(text.contains("persistent credentials"));
    assert!(text.contains("genuine HAP setup code"));
    let help = run(&["pair", "--help"]);
    assert!(String::from_utf8(help.stdout)
        .unwrap()
        .contains("selected method"));
}

#[test]
fn record_diagnostic_accepts_allowlisted_id_but_requires_terminal_before_discovery() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("piair.toml");
    fs::write(&config, "receiver_allowlist=['device:aabbccddeeff']").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_piair"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "stream-test",
            "device:aabbccddeeff",
            "--record-check",
            "--experimental-owntone-transient",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
    assert!(!dir.path().join("receiver-state").exists());
}

#[test]
fn stream_diagnostics_require_terminal_without_state_or_contact() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("piair.toml");
    std::fs::write(&config, "receiver_allowlist=['device:aabbccddeeff']").unwrap();
    for mode in ["--stream-setup-check", "--playback-check"] {
        let output = Command::new(env!("CARGO_BIN_EXE_piair"))
            .args([
                "--config",
                config.to_str().unwrap(),
                "stream-test",
                "device:aabbccddeeff",
                mode,
                "--experimental-owntone-transient",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
        assert!(!dir.path().join("receiver-state").exists());
    }
}

#[test]
fn hap_diagnostics_and_legacy_aliases_require_authorization_without_contact() {
    for mode in [
        "hap-stream-setup-check",
        "hap-playback-check",
        "stua-stream-setup-check",
        "stua-playback-check",
    ] {
        let picker = run(&[mode]);
        assert_eq!(picker.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&picker.stderr).contains("interactive terminal"));
        for id in ["Stua", "device:02D899C1833F", "device:02d899c1833f:7000"] {
            let output = run(&[mode, id]);
            assert_eq!(output.status.code(), Some(1));
            assert!(String::from_utf8(output.stderr)
                .unwrap()
                .contains("exact stable device:"));
        }
        for flag in [
            "--seconds=1",
            "--experimental-owntone-transient",
            "--setup-profile=a",
            "--playback-check",
        ] {
            assert_eq!(
                run(&[mode, "device:02d899c1833f", flag]).status.code(),
                Some(2)
            );
        }
        let output = run(&[mode, "device:02d899c1833f", "--volume-db=-40"]);
        assert_eq!(
            output.status.code(),
            Some(if mode.ends_with("playback-check") {
                1
            } else {
                2
            })
        );
        for id in ["device:112233445566", "device:66778899aabb"] {
            let output = run(&[mode, id]);
            assert_eq!(output.status.code(), Some(1));
            assert!(String::from_utf8(output.stderr)
                .unwrap()
                .contains("not approved"));
        }
    }
}

#[test]
fn stored_hap_operation_method_cannot_be_selected_for_enrollment() {
    assert_eq!(
        run(&["select", "device:02d899c1833f", "--method", "stored-hap"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn passive_receiver_plan_help_and_auth_intent_contract_are_device_free() {
    let help = run(&["receiver-plan", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("no receiver connections or state writes"));
    assert!(help.contains("--method"));
    for args in [
        vec!["receiver-plan", "--method", "auto"],
        vec!["receiver-plan", "--method", "stored-hap"],
        vec!["receiver-plan", "device:aabbccddeeff"],
        vec!["receiver-plan", "--play"],
    ] {
        assert_eq!(run(&args).status.code(), Some(2));
    }
}

#[test]
fn buffered_audio_intent_fails_locally_without_selection_discovery_or_confirmation() {
    for extra in [vec![], vec!["--confirm-audio"]] {
        let args: Vec<_> = ["buffered-test", "device:112233445566"]
            .into_iter()
            .chain(extra)
            .collect();
        let output = run(&args);
        assert!(!output.status.success());
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .contains("clock adoption is unverified"));
    }
    assert!(run(&["buffered-test", "--help"]).status.success());
    assert_eq!(run(&["buffered-test"]).status.code(), Some(2));
    assert_eq!(
        run(&[
            "buffered-test",
            "device:112233445566",
            "--setup-check",
            "--confirm-audio"
        ])
        .status
        .code(),
        Some(2)
    );
    assert!(
        !run(&["buffered-test", "device:112233445566", "--setup-check"])
            .status
            .success()
    );
}

#[test]
fn buffered_control_checks_are_explicit_and_noninteractive_runs_fail_closed() {
    for stage in ["record", "stream", "anchor"] {
        let output = run(&[
            "buffered-test",
            "device:112233445566",
            "--control-check",
            stage,
        ]);
        assert!(!output.status.success());
        for conflict in ["--confirm-audio", "--setup-check"] {
            assert_eq!(
                run(&[
                    "buffered-test",
                    "device:112233445566",
                    "--control-check",
                    stage,
                    conflict
                ])
                .status
                .code(),
                Some(2)
            );
        }
    }
    assert_eq!(
        run(&[
            "buffered-test",
            "device:112233445566",
            "--control-check",
            "play"
        ])
        .status
        .code(),
        Some(2)
    );
}

#[test]
fn wav_diagnostic_arguments_fail_before_receiver_contact() {
    let base = [
        "stream-test",
        "device:0a05559f132f",
        "--experimental-owntone-transient",
        "--audio-file",
        "tests/assets/audio/Pachelbel_Canon_audio_test.wav",
    ];
    for extra in [
        vec!["--seconds", "50"],
        vec!["--volume-db", "-9"],
        vec!["--playback-check"],
        vec!["--setup-check"],
        vec!["--record-check"],
        vec!["--stream-setup-check"],
    ] {
        let args: Vec<_> = base.iter().copied().chain(extra).collect();
        assert_eq!(run(&args).status.code(), Some(2));
    }
    let bad = run(&[
        "stream-test",
        "--experimental-owntone-transient",
        "--audio-file",
        "/no-such-piair-test.wav",
    ]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("open WAV"));
}

#[test]
fn stored_hap_wav_accepts_full_command_but_requires_interactive_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "receiver_allowlist=['device:02d899c1833f']").unwrap();
    let output = run(&[
        "--config",
        config.to_str().unwrap(),
        "stream-test",
        "device:02d899c1833f",
        "--audio-file",
        "tests/assets/audio/Pachelbel_Canon_audio_test.wav",
        "--seconds",
        "30",
        "--volume-db",
        "-20",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("interactive terminal and explicit confirmation"));
    let invalid = run(&["stream-test", "--audio-file", "/no-such-piair-test.wav"]);
    assert_eq!(invalid.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("open WAV"));
}

#[test]
fn automatic_capture_requires_explicit_receiver_device_and_valid_durations() {
    let help = run(&["automatic-ptp-capture", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    for flag in [
        "--threshold-dbfs",
        "--start-duration-ms",
        "--stop-duration-ms",
        "--experimental-owntone-transient",
    ] {
        assert!(help.contains(flag));
    }
    assert_eq!(run(&["automatic-ptp-capture"]).status.code(), Some(2));
    assert_eq!(
        run(&["automatic-ptp-capture", "device:112233445566"])
            .status
            .code(),
        Some(2)
    );
    for (flag, value) in [
        ("--start-duration-ms", "0"),
        ("--start-duration-ms", "9001"),
        ("--stop-duration-ms", "120001"),
    ] {
        assert_eq!(
            run(&[
                "automatic-ptp-capture",
                "device:112233445566",
                "--capture-device",
                "never-open",
                flag,
                value
            ])
            .status
            .code(),
            Some(2)
        );
    }
    let output = run(&[
        "automatic-ptp-capture",
        "device:112233445566",
        "--capture-device",
        "never-open",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("receiver is not approved"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("ALSA"));
}

#[test]
fn unattended_run_requires_opt_in_receiver_and_explicit_authentication() {
    let base = [
        "run",
        "device:aabbccddeeff",
        "--capture-device",
        "synthetic-only",
    ];
    for extra in [
        vec![],
        vec!["--authentication", "own-tone-transient"],
        vec!["--allow-unattended", "--authentication", "legacy"],
    ] {
        let args: Vec<_> = base.into_iter().chain(extra).collect();
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("ALSA"));
    }
    let output = run(&base
        .into_iter()
        .chain(["--allow-unattended"])
        .collect::<Vec<_>>());
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing playback.authentication"));
    let output = run(&["run", "--help"]);
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--allow-unattended"));
    assert!(help.contains("stored-hap"));
    assert!(help.contains("own-tone-transient"));
}

#[test]
fn run_without_approval_fails_without_tty_prompt_or_device_access() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "receiver_allowlist=[]").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_piair"))
        .arg("--config")
        .arg(&config)
        .args([
            "run",
            "device:aabbccddeeff",
            "--allow-unattended",
            "--capture-device",
            "synthetic-only",
            "--authentication",
            "own-tone-transient",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("receiver is not approved"));
    assert!(!error.contains("ALSA"));
    assert!(!config.parent().unwrap().join("receiver-state").exists());
}

#[test]
fn no_arguments_rejects_noninteractive_input() {
    let output = run(&[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires an interactive terminal"));
}
#[test]
fn daily_commands_help_and_missing_service_are_actionable() {
    for command in ["start", "stop", "restart", "logs"] {
        assert!(run(&[command, "--help"]).status.success());
        let output = run(&[command]);
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("systemd")
                || error.contains("non-root")
                || error.contains("not installed")
        );
    }
    assert!(run(&["logs", "--follow", "--help"]).status.success());
}

#[test]
fn ordinary_help_focuses_on_daily_use_and_keeps_manual_tools() {
    let output = run(&["--help"]);
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("daily menu"));
    assert!(help.contains("piair audio devices"));
    for command in [
        "setup",
        "status",
        "start",
        "stop",
        "restart",
        "logs",
        "devices",
        "audio",
        "config",
        "update",
        "uninstall",
        "allowlist",
        "select",
        "pair",
        "unpair",
    ] {
        assert!(
            help.lines()
                .any(|line| line.trim_start().starts_with(&format!("{command} "))),
            "{help}"
        );
    }
    assert!(
        help.lines().count() <= 50,
        "top-level help should fit a short terminal view: {help}"
    );
}

#[test]
fn hidden_diagnostics_keep_their_own_help_arguments_and_aliases() {
    let ordinary = String::from_utf8(run(&["--help"]).stdout).unwrap();
    for (command, argument) in [
        ("run", "--allow-unattended"),
        ("receiver-plan", "--method"),
        ("authenticate", "--control-check"),
        ("stream-test", "--audio-file"),
        ("ptp-negotiation-check", "--observation-seconds"),
        ("ptp-wav-playback", "--audio-file"),
        ("experimental-ptp-capture", "--capture-device"),
        ("automatic-ptp-capture", "--threshold-dbfs"),
        ("buffered-test", "--confirm-audio"),
        ("hap-stream-setup-check", "[RECEIVER_ID]"),
        ("hap-playback-check", "--volume-db"),
        ("stua-stream-setup-check", "[RECEIVER_ID]"),
        ("stua-playback-check", "--volume-db"),
    ] {
        assert!(!ordinary
            .lines()
            .any(|line| line.trim_start().starts_with(&format!("{command} "))));
        let output = run(&[command, "--help"]);
        assert!(output.status.success(), "{command}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(argument),
            "{command}"
        );
        assert_eq!(run(&[command, "--unknown-option"]).status.code(), Some(2));
    }
    for command in ["update", "uninstall"] {
        assert!(run(&[command, "--help"]).status.success());
    }
}

#[test]
fn command_errors_are_readable_except_in_unattended_json_mode() {
    for verbosity in [None, Some("-v"), Some("-vv")] {
        for unattended in [false, true] {
            let mut args = vec!["--config", "/nonexistent/piair.toml"];
            if let Some(flag) = verbosity {
                args.push(flag);
            }
            if unattended {
                args.extend(["run", "--allow-unattended"]);
            } else {
                args.extend(["config", "check"]);
            }
            let output = run(&args);
            assert_eq!(output.status.code(), Some(1));
            let error = String::from_utf8(output.stderr).unwrap();
            assert!(error.contains("check path and read permissions"));
            if unattended {
                let event: serde_json::Value = serde_json::from_str(error.trim()).unwrap();
                assert_eq!(event["level"], "ERROR");
            } else {
                assert!(error.contains("ERROR"));
                assert!(!error.contains("\"fields\""));
            }
        }
    }
}
