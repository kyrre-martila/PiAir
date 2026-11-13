use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
const ID: &str = "device:aabbccddeeff";

fn command(path: &Path, action: &str, id: Option<&str>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_piair"));
    command
        .arg("--config")
        .arg(path)
        .args(["allowlist", action]);
    if let Some(id) = id {
        command.arg(id);
    }
    command
}

struct Terminal {
    child: Child,
    master: File,
    output: String,
}
impl Terminal {
    fn start(mut command: Command) -> Self {
        use rustix::pty::*;
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
        command
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        Self {
            child: command.spawn().unwrap(),
            master: master.into(),
            output: String::new(),
        }
    }
    fn until(&mut self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.output.contains(needle) {
            let mut bytes = [0; 4096];
            match self.master.read(&mut bytes) {
                Ok(n) => self.output.push_str(&String::from_utf8_lossy(&bytes[..n])),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("PTY failed: {e}: {}", self.output),
            }
            assert!(
                Instant::now() < deadline,
                "waiting for {needle}: {}",
                self.output
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn answer(&mut self, text: &str) {
        self.master.write_all(text.as_bytes()).unwrap();
    }
    fn finish(&mut self, success: bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert_eq!(status.success(), success, "{}", self.output);
                break;
            }
            assert!(Instant::now() < deadline, "child did not exit");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn terminal_success_preservation_duplicate_removal_and_absence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let source = "# keep\nreceiver_allowlist = ['device:112233445566'] # approved\n[audio]\ndevice = 'do-not-open' # capture\n";
    fs::write(&path, source).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let mut terminal = Terminal::start(command(&path, "add", Some(ID)));
    terminal.until("y/yes to confirm");
    assert!(terminal.output.contains(&format!("Receiver ID: {ID}")));
    assert!(!terminal.output.contains("physically"));
    assert!(!terminal.output.contains("device label"));
    terminal.answer("y\n");
    terminal.finish(true);
    let added = fs::read_to_string(&path).unwrap();
    for preserved in [
        "# keep",
        "'device:112233445566'",
        "# approved",
        "[audio]\ndevice = 'do-not-open' # capture",
    ] {
        assert!(added.contains(preserved), "{added}");
    }
    assert!(added.contains(ID));
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let duplicate = command(&path, "add", Some(ID)).output().unwrap();
    assert!(duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stdout).contains("already approved"));
    assert_eq!(fs::read_to_string(&path).unwrap(), added);
    let list = command(&path, "list", None).output().unwrap();
    assert!(list.status.success());
    assert!(String::from_utf8_lossy(&list.stdout).contains(ID));
    let mut terminal = Terminal::start(command(&path, "remove", Some(ID)));
    terminal.until("Remove approval for");
    terminal.answer("yes\n");
    terminal.finish(true);
    assert!(!fs::read_to_string(&path).unwrap().contains(ID));
    let absent = command(&path, "remove", Some(ID)).output().unwrap();
    assert!(absent.status.success());
    assert!(String::from_utf8_lossy(&absent.stdout).contains("absent"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn cancellation_stale_write_and_readonly_leave_file_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "# original\n").unwrap();
    for answer in ["n\n", "no\n", "\n", "\x04"] {
        let mut terminal = Terminal::start(command(&path, "add", Some(ID)));
        terminal.until("Approve this AirPlay receiver");
        terminal.answer(answer);
        terminal.finish(true);
        assert_eq!(fs::read_to_string(&path).unwrap(), "# original\n");
    }
    let mut terminal = Terminal::start(command(&path, "add", Some(ID)));
    terminal.until("Approve this AirPlay receiver");
    fs::write(&path, "# newer\n").unwrap();
    terminal.answer("yes\n");
    terminal.finish(false);
    assert_eq!(fs::read_to_string(&path).unwrap(), "# newer\n");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
    let mut terminal = Terminal::start(command(&path, "add", Some(ID)));
    terminal.until("Approve this AirPlay receiver");
    terminal.answer("yes\n");
    terminal.finish(false);
    assert_eq!(fs::read_to_string(&path).unwrap(), "# newer\n");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn invalid_ambiguous_ids_configs_and_pipes_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "").unwrap();
    for id in [
        "room",
        "192.0.2.1",
        "room.local",
        "device:AABBCCDDEEFF",
        "device:000000000000",
        "device:ffffffffffff",
        "device:aa:bb:cc:dd:ee:ff",
        "device:aabbccddeeff:7000",
        "device:aabbccddeefg",
    ] {
        for action in ["add", "remove"] {
            assert!(!command(&path, action, Some(id))
                .output()
                .unwrap()
                .status
                .success());
        }
    }
    assert!(!command(&path, "add", Some(ID))
        .output()
        .unwrap()
        .status
        .success());
    for source in [
        "receiver_allowlist=['device:aabbccddeeff','device:aabbccddeeff']",
        "receiver_allowlist=['Living Room']",
        "receiver_allowlist=5",
        "discovery_timeout_secs=0",
        "broken = [",
    ] {
        fs::write(&path, source).unwrap();
        for action in ["list", "add", "remove"] {
            assert!(!command(&path, action, (action != "list").then_some(ID))
                .output()
                .unwrap()
                .status
                .success());
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
    }
    assert!(!dir.path().join("receiver-state").exists());
}

#[test]
fn resolution_and_missing_files_follow_existing_rules() {
    let dir = tempfile::tempdir().unwrap();
    let explicit = dir.path().join("explicit.toml");
    let env = dir.path().join("env.toml");
    fs::write(&explicit, "receiver_allowlist=['device:112233445566']").unwrap();
    fs::write(&env, format!("receiver_allowlist=['{ID}']")).unwrap();
    let result = command(&explicit, "list", None)
        .env("PIAIR_CONFIG", &env)
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(!String::from_utf8_lossy(&result.stdout).contains(ID));
    let result = Command::new(env!("CARGO_BIN_EXE_piair"))
        .args(["allowlist", "list"])
        .env("PIAIR_CONFIG", &env)
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains(ID));
    assert!(!command(&dir.path().join("missing"), "list", None)
        .output()
        .unwrap()
        .status
        .success());
    let mut default = Command::new(env!("CARGO_BIN_EXE_piair"));
    default
        .env_remove("PIAIR_CONFIG")
        .env("XDG_CONFIG_HOME", dir.path())
        .args(["allowlist", "add", ID]);
    let mut terminal = Terminal::start(default);
    terminal.until("Approve this AirPlay receiver");
    terminal.answer("yes\n");
    terminal.finish(true);
    assert!(fs::read_to_string(dir.path().join("piair/config.toml"))
        .unwrap()
        .contains(ID));
}

#[test]
fn removal_cancellation_and_unwritable_directory_preserve_original() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let source = format!("receiver_allowlist=['{ID}'] # keep\n");
    fs::write(&path, &source).unwrap();
    let mut terminal = Terminal::start(command(&path, "remove", Some(ID)));
    terminal.until("Remove approval for");
    terminal.answer("no\n");
    terminal.finish(true);
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let mut terminal = Terminal::start(command(&path, "remove", Some(ID)));
    terminal.until("Remove approval for");
    terminal.answer("yes\n");
    terminal.finish(false);
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn allowlist_help_describes_security_and_commands() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing");
    let result = command(&path, "--help", None).output().unwrap();
    assert!(result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    for expected in [
        "list",
        "add",
        "remove",
        "device:aabbccddeeff",
        "one explicit approval",
        "does not authenticate",
        "y/yes",
    ] {
        assert!(text.contains(expected), "{text}");
    }
    assert!(!text.contains("physical identity verification"));
    assert!(!text.contains("device label"));
}
