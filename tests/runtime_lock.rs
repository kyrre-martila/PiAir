//! Actual CLI processes with synthetic ALSA null capture; never contact a receiver.
use std::{
    fs::{self, File},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
const ID: &str = "device:aabbccddeeff";

fn fixture(root: &Path) -> PathBuf {
    fs::create_dir_all(root).unwrap();
    let config = root.join("config.toml");
    fs::write(&config, format!("receiver_allowlist=['{ID}']")).unwrap();
    let state = root.join("receiver-state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let selection = state.join("selected.toml");
    fs::write(
        &selection,
        format!(
            r#"
id = "{ID}"
method = "transient"
addresses = ["127.0.0.1"]
port = 9
[capabilities]
features = "0001420000080200"
flags = 0
"#
        ),
    )
    .unwrap();
    fs::set_permissions(selection, fs::Permissions::from_mode(0o600)).unwrap();
    config
}
fn command(config: &Path, automatic: bool, device: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_piair"));
    command.arg("--config").arg(config);
    if automatic {
        command.args([
            "automatic-ptp-capture",
            ID,
            "--experimental-owntone-transient",
        ]);
    } else {
        command.args([
            "run",
            ID,
            "--allow-unattended",
            "--authentication",
            "own-tone-transient",
        ]);
    }
    command.args(["--capture-device", device, "--threshold-dbfs", "0"]);
    command
}
struct Process {
    child: Child,
    log: PathBuf,
}
impl Process {
    fn spawn(mut command: Command, log: PathBuf) -> Self {
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(&log).unwrap())
            .spawn()
            .unwrap();
        Self { child, log }
    }
    fn output(&self) -> String {
        fs::read_to_string(&self.log).unwrap()
    }
    fn ready(&mut self) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "early exit: {}",
                self.output()
            );
            if self.output().contains("\"event\":\"monitoring\"") {
                return;
            }
            assert!(Instant::now() < end, "startup timed out: {}", self.output());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn wait(&mut self) -> ExitStatus {
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < end,
                "process did not exit promptly: {}",
                self.output()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn signal(&mut self, signal: &str) -> ExitStatus {
        assert!(Command::new("kill")
            .args([signal, &self.child.id().to_string()])
            .status()
            .unwrap()
            .success());
        self.wait()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn automatic_processes_exclude_each_other_and_kernel_releases_lock_on_exit() {
    let dir = tempfile::tempdir().unwrap();
    let first_config = fixture(&dir.path().join("first"));
    let other_config = fixture(&dir.path().join("other"));
    for signal in ["-INT", "-TERM", "-KILL"] {
        let mut first = Process::spawn(
            command(&first_config, false, "null"),
            dir.path().join("first.log"),
        );
        first.ready();
        for automatic in [false, true] {
            // Different config paths and devices must not bypass exclusion.
            // The invalid device also proves rejection precedes ALSA opening.
            let mut second = Process::spawn(
                command(&other_config, automatic, "piair-never-open"),
                dir.path().join("second.log"),
            );
            assert_eq!(second.wait().code(), Some(1));
            let error = second.output();
            assert!(
                error.contains("another automatic playback instance is already running"),
                "{error}"
            );
            assert!(error.contains("stop that piair run or automatic-ptp-capture"));
            assert!(!error.contains("ALSA"));
            assert!(!error.contains("interactive confirmation"));
            assert!(
                first.child.try_wait().unwrap().is_none(),
                "contender interrupted owner"
            );
        }
        let status = first.signal(signal);
        if signal == "-KILL" {
            assert!(!status.success());
        } else {
            assert!(status.success(), "{}", first.output());
            assert!(first.output().contains("\"event\":\"shutdown\""));
        }
        // A successor acquires immediately, even following uncatchable SIGKILL.
        let mut successor = Process::spawn(
            command(&other_config, false, "null"),
            dir.path().join("successor.log"),
        );
        successor.ready();
        assert!(successor.signal("-TERM").success());
    }
    // Startup/capture failure must release the lock too, with a failing exit code.
    let mut failed = Process::spawn(
        command(&first_config, false, "piair-never-open"),
        dir.path().join("failure.log"),
    );
    assert_eq!(failed.wait().code(), Some(1));
    assert!(failed.output().contains("ALSA capture failed"));
    let mut successor = Process::spawn(
        command(&other_config, false, "null"),
        dir.path().join("after-error.log"),
    );
    successor.ready();
    assert!(successor.signal("-TERM").success());
    // The same process/authorization path works with persisted settings alone.
    let output = Command::new(env!("CARGO_BIN_EXE_piair"))
        .arg("--config")
        .arg(&first_config)
        .args([
            "config",
            "set-playback",
            ID,
            "--capture-device",
            "null",
            "--authentication",
            "own-tone-transient",
            "--threshold-dbfs",
            "0",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let missing_opt_in = Command::new(env!("CARGO_BIN_EXE_piair"))
        .arg("--config")
        .arg(&first_config)
        .arg("run")
        .output()
        .unwrap();
    assert_eq!(missing_opt_in.status.code(), Some(2));
    let mut saved_command = Command::new(env!("CARGO_BIN_EXE_piair"));
    saved_command
        .arg("--config")
        .arg(&first_config)
        .args(["run", "--allow-unattended"]);
    let mut saved = Process::spawn(saved_command, dir.path().join("saved.log"));
    saved.ready();
    let text = fs::read_to_string(&first_config).unwrap();
    fs::write(
        &first_config,
        text.replace("threshold_dbfs = 0.0", "threshold_dbfs = -48.0"),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert!(saved.child.try_wait().unwrap().is_none());
    fs::write(&first_config, "receiver_allowlist=[]").unwrap();
    assert_eq!(saved.wait().code(), Some(1));
    assert!(saved.output().contains("approval revoked"));
    for root in ["first", "other"] {
        let state = dir.path().join(root).join("receiver-state");
        assert_eq!(
            fs::read_dir(state).unwrap().count(),
            1,
            "no lock/PID artifacts"
        );
    }
}
