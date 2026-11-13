use super::*;
use crate::receiver::{
    compatibility::{Capabilities, Method},
    store::Store,
};
use std::{collections::VecDeque, fs};
const ID: &str = "device:aabbccddeeff";
fn settings(authentication: Authentication) -> Settings {
    Settings {
        capture_device: "synthetic-only".into(),
        receiver_id: ID.into(),
        authentication,
        detection: activity::Settings {
            threshold_dbfs: -45.0,
            start_ms: 1000,
            stop_ms: 30000,
        },
        volume_db: None,
    }
}
fn fixture(method: Method) -> (tempfile::TempDir, Loaded, Selection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, format!("receiver_allowlist=['{ID}']")).unwrap();
    let selected = Selection {
        id: ID.into(),
        advertised_pi: None,
        method,
        addresses: ["127.0.0.1".parse().unwrap()].into(),
        port: 7000,
        capabilities: Capabilities {
            features: (1 << 9) | (1 << 19) | (1 << 41) | (1 << 46) | (1 << 48),
            flags: Some(0),
            password: None,
            access_control: None,
            access_control_type: None,
            protocol_version: None,
        },
    };
    Store::new(&path).unwrap().select(&selected).unwrap();
    let loaded = config::load(Some(path)).unwrap();
    (dir, loaded, selected)
}
fn credential() -> store::Credential {
    store::Credential {
        version: 2,
        receiver_id: ID.into(),
        advertised_pi: None,
        pairing_id: "fixture-receiver".into(),
        controller_id: "00000000-0000-4000-8000-000000000001".into(),
        seed: [11; 32],
        receiver_key: ed25519_dalek::SigningKey::from_bytes(&[22; 32])
            .verifying_key()
            .to_bytes(),
    }
}
#[test]
fn opt_in_is_required_before_any_state_access_or_signal_registration() {
    let (_dir, loaded, _) = fixture(Method::Transient);
    assert!(
        unattended(&loaded, settings(Authentication::OwnToneTransient), false)
            .unwrap_err()
            .to_string()
            .contains("--allow-unattended")
    );
}
#[test]
fn preparation_requires_current_approval_exact_saved_selection_and_explicit_authentication() {
    let (_dir, loaded, selected) = fixture(Method::Transient);
    let before = fs::read_dir(loaded.path.parent().unwrap()).unwrap().count();
    assert!(Runtime::prepare(&loaded, settings(Authentication::OwnToneTransient)).is_ok());
    assert!(Runtime::prepare(&loaded, settings(Authentication::StoredHap)).is_err());
    let mut wrong = settings(Authentication::OwnToneTransient);
    wrong.receiver_id = "device:112233445566".into();
    assert!(Runtime::prepare(&loaded, wrong).is_err());
    let store = Store::new(&loaded.path).unwrap();
    let mut changed = selected;
    changed.method = Method::PinDisplay;
    store.select(&changed).unwrap();
    assert!(Runtime::prepare(&loaded, settings(Authentication::OwnToneTransient)).is_err());
    assert!(Runtime::prepare(&loaded, settings(Authentication::StoredHap)).is_err());
    store.save(&credential()).unwrap();
    assert!(Runtime::prepare(&loaded, settings(Authentication::StoredHap)).is_ok());
    fs::write(&loaded.path, "receiver_allowlist=[]").unwrap();
    assert!(Runtime::prepare(&loaded, settings(Authentication::StoredHap)).is_err());
    assert_eq!(
        fs::read_dir(loaded.path.parent().unwrap()).unwrap().count(),
        before
    );
}
#[test]
fn idle_runtime_detects_revoked_approval_changed_selection_and_credentials() {
    for change in [
        "approval",
        "selection",
        "credentials",
        "corrupt-credentials",
        "corrupt-config",
    ] {
        let (_dir, loaded, mut selected) = fixture(Method::PinDisplay);
        let store = Store::new(&loaded.path).unwrap();
        store.save(&credential()).unwrap();
        let runtime = Runtime::prepare(&loaded, settings(Authentication::StoredHap)).unwrap();
        match change {
            "approval" => fs::write(&loaded.path, "receiver_allowlist=[]").unwrap(),
            "selection" => {
                selected.port += 1;
                store.select(&selected).unwrap();
            }
            "credentials" => store.remove(ID).unwrap(),
            "corrupt-credentials" => fs::write(
                loaded
                    .path
                    .parent()
                    .unwrap()
                    .join("receiver-state/aabbccddeeff.toml"),
                "invalid",
            )
            .unwrap(),
            _ => fs::write(&loaded.path, "invalid").unwrap(),
        }
        assert!(runtime.check().is_err(), "{change}");
    }
}
#[test]
fn invalid_runtime_settings_fail_before_capture_or_discovery() {
    let (_dir, loaded, _) = fixture(Method::Transient);
    for invalid in ["volume", "device", "threshold", "start", "stop"] {
        let mut settings = settings(Authentication::OwnToneTransient);
        match invalid {
            "volume" => settings.volume_db = Some(-9),
            "device" => settings.capture_device.clear(),
            "threshold" => settings.detection.threshold_dbfs = f64::NAN,
            "start" => settings.detection.start_ms = 0,
            _ => settings.detection.stop_ms = 0,
        }
        assert!(Runtime::prepare(&loaded, settings).is_err());
    }
}

struct Synthetic {
    start: Instant,
    seconds: u64,
    stop: u64,
    cancel: Arc<AtomicBool>,
    meaningful: bool,
    failures: VecDeque<bool>,
    attempts: Vec<u64>,
    rearms: usize,
    fatal: bool,
}
impl Driver for Synthetic {
    fn check(&mut self) -> Result<()> {
        ensure!(!self.fatal, "fatal capture failure");
        Ok(())
    }
    fn meaningful(&self) -> Result<bool> {
        Ok(self.meaningful)
    }
    fn attempt(&mut self) -> Result<()> {
        self.attempts.push(self.seconds);
        let failed = self.failures.pop_front().unwrap_or(true);
        if !failed {
            self.meaningful = false;
        }
        ensure!(!failed, "recoverable session failure");
        Ok(())
    }
    fn rearm(&mut self) -> Result<()> {
        self.rearms += 1;
        Ok(())
    }
    fn now(&self) -> Instant {
        self.start + Duration::from_secs(self.seconds)
    }
    fn wait(&mut self) {
        self.seconds += 1;
        if self.seconds == 35 {
            self.meaningful = true;
        }
        if self.seconds >= self.stop {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }
}
fn synthetic(cancel: Arc<AtomicBool>) -> Synthetic {
    Synthetic {
        start: Instant::now(),
        seconds: 0,
        stop: 600,
        cancel,
        meaningful: true,
        failures: VecDeque::new(),
        attempts: vec![],
        rearms: 0,
        fatal: false,
    }
}
#[test]
fn repeated_failures_use_existing_bounded_exponential_backoff() {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut driver = synthetic(cancel.clone());
    supervise(&mut driver, &cancel).unwrap();
    assert_eq!(&driver.attempts[..7], &[0, 2, 6, 14, 30, 62, 122]);
    assert!(driver.attempts.len() < 16);
    assert_eq!(driver.attempts.len(), driver.rearms);
}
#[test]
fn silence_never_dials_and_new_sustained_audio_resumes_after_clean_stop() {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut driver = synthetic(cancel.clone());
    driver.meaningful = false;
    driver.stop = 100;
    driver.failures = [false, false].into();
    supervise(&mut driver, &cancel).unwrap();
    assert_eq!(driver.attempts, [35]);
    assert_eq!(driver.rearms, 0);
    // A later detection epoch resumes using the same orchestration owner.
    cancel.store(false, Ordering::Relaxed);
    driver.meaningful = true;
    driver.stop = 101;
    supervise(&mut driver, &cancel).unwrap();
    assert_eq!(driver.attempts, [35, 100]);
}
#[test]
fn fatal_capture_error_and_preexisting_cancellation_do_not_attempt_sessions() {
    for fatal in [false, true] {
        let cancel = Arc::new(AtomicBool::new(true));
        let mut driver = synthetic(cancel.clone());
        driver.fatal = fatal;
        assert_eq!(supervise(&mut driver, &cancel).is_err(), fatal);
        assert!(driver.attempts.is_empty());
    }
}

#[test]
fn signal_child() {
    let Ok(marker) = std::env::var("PIAIR_RUNTIME_SIGNAL_TEST") else {
        return;
    };
    use std::io::IsTerminal;
    assert!(!std::io::stdin().is_terminal());
    assert!(!std::io::stdout().is_terminal());
    let (_dir, loaded, _) = fixture(Method::Transient);
    let runtime = Runtime::prepare(&loaded, settings(Authentication::OwnToneTransient)).unwrap();
    let cancel = signal_cancellation().unwrap();
    let closed = Arc::new(AtomicBool::new(false));
    let capture = crate::audio::live::tests::mock(closed.clone(), true, None);
    capture
        .enable_automatic(runtime.settings.detection)
        .unwrap();
    let mut engine = Engine {
        runtime,
        capture,
        cancel: cancel.clone(),
    };
    fs::write(&marker, "ready").unwrap();
    supervise(&mut engine, &cancel).unwrap();
    drop(engine);
    assert!(closed.load(Ordering::Acquire));
    fs::write(marker, "joined").unwrap();
}
#[test]
fn sigterm_and_sigint_without_tty_stop_monitoring_and_join_capture() {
    use std::process::{Command, Stdio};
    for signal in ["-TERM", "-INT"] {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("signal");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "receiver::runtime::tests::signal_child",
                "--nocapture",
            ])
            .env("PIAIR_RUNTIME_SIGNAL_TEST", &marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            assert!(
                child.try_wait().unwrap().is_none(),
                "child exited before readiness"
            );
            std::thread::park_timeout(Duration::from_millis(5));
        }
        if !marker.exists() {
            child.kill().unwrap();
            panic!("child readiness timed out");
        }
        assert!(Command::new("kill")
            .args([signal, &child.id().to_string()])
            .status()
            .unwrap()
            .success());
        let deadline = Instant::now() + Duration::from_secs(3);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                panic!("signal shutdown timed out");
            }
            std::thread::park_timeout(Duration::from_millis(5));
        };
        assert!(status.success());
        assert_eq!(fs::read_to_string(marker).unwrap(), "joined");
    }
}

#[test]
fn hap_runtime_network_lease_keeps_persisted_selection_and_credential_checks() {
    use crate::{
        discovery::{Advertisement, Receiver},
        receiver::crypto::ControlKeys,
    };
    use std::net::TcpListener;
    let (_dir, loaded, mut selected) = fixture(Method::PinDisplay);
    let store = Store::new(&loaded.path).unwrap();
    store.save(&credential()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    selected.port = listener.local_addr().unwrap().port();
    store.select(&selected).unwrap();
    let info = mdns_sd::ServiceInfo::new(
        "_airplay._tcp.local.",
        "Fixture",
        "fixture.local.",
        "127.0.0.1",
        selected.port,
        [
            ("deviceid", "AA:BB:CC:DD:EE:FF"),
            ("features", "0x80200,0x14200"),
            ("flags", "0"),
        ]
        .as_slice(),
    )
    .unwrap();
    let snapshot = [Receiver {
        id: ID.into(),
        name: "Fixture".into(),
        services: vec![Advertisement::test_from_info(&info)],
    }];
    let runtime = Runtime::prepare(&loaded, settings(Authentication::StoredHap)).unwrap();
    let wire = wire::Wire::connect_hap_scoped(
        &loaded.path,
        &runtime.operation,
        &snapshot,
        runtime.credential.as_ref().unwrap(),
        Some(&selected),
    )
    .unwrap();
    let (peer, _) = listener.accept().unwrap();
    let wire = wire
        .secure(
            ControlKeys::new(b"runtime-test"),
            ControlKeys::new(b"event-test"),
        )
        .unwrap();
    let lease = wire.ptp_lease().unwrap();
    lease.check().unwrap();
    let mut changed = selected.clone();
    changed.port += 1;
    store.select(&changed).unwrap();
    assert!(lease
        .check()
        .unwrap_err()
        .to_string()
        .contains("selection changed"));
    store.select(&selected).unwrap();
    lease.check().unwrap();
    store.remove(ID).unwrap();
    assert!(lease.check().is_err());
    drop((wire, peer));
}

#[test]
fn interactive_automatic_entry_still_requires_terminal_authorization() {
    let (_dir, loaded, _) = fixture(Method::Transient);
    let state = loaded
        .path
        .parent()
        .unwrap()
        .join("receiver-state/selected.toml");
    let before = fs::read(&state).unwrap();
    let error = crate::receiver::automatic_capture(
        &loaded,
        ID,
        "never-open",
        None,
        true,
        settings(Authentication::OwnToneTransient).detection,
    )
    .unwrap_err();
    assert!(error.to_string().contains("interactive confirmation"));
    assert_eq!(fs::read(state).unwrap(), before);
}
#[test]
fn cancellation_during_backoff_does_not_wait_for_retry_deadline() {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut driver = synthetic(cancel.clone());
    driver.stop = 1;
    supervise(&mut driver, &cancel).unwrap();
    assert_eq!(driver.attempts, [0]);
    assert_eq!(driver.seconds, 1);
}

#[test]
fn cancellation_does_not_mask_bounded_teardown_failure() {
    assert!(finish_attempt(Err(anyhow::anyhow!("playback interrupted")), true).is_ok());
    assert!(finish_attempt(Err(anyhow::anyhow!("connection failed")), false).is_err());
    let cleanup = anyhow::Error::new(crate::receiver::lifecycle::CleanupFailure(anyhow::anyhow!(
        "TEARDOWN acknowledgement timed out"
    )))
    .context("playback interrupted");
    let error = finish_attempt(Err(cleanup), true).unwrap_err();
    assert!(error.is::<crate::receiver::lifecycle::CleanupFailure>());
    assert!(format!("{error:#}").contains("TEARDOWN acknowledgement timed out"));
}

#[test]
fn sigint_and_sigterm_during_active_airplay_join_workers_and_preserve_cleanup_failure() {
    use std::process::{Command, Stdio};
    for signal in ["-INT", "-TERM"] {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "receiver::lifecycle::tests::playback_cancellation_timing_loss_and_receiver_disconnect_join_and_cleanup", "--nocapture"])
            .env("PIAIR_ACTIVE_PLAYBACK_SIGNAL", signal)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit())
            .spawn().unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "active {signal} shutdown regression");
                break;
            }
            if Instant::now() >= end {
                let _ = child.kill();
                let _ = child.wait();
                panic!("active {signal} shutdown exceeded bound");
            }
            std::thread::park_timeout(Duration::from_millis(5));
        }
    }
}
