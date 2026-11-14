use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::ffi::OsStrExt,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
// Wizard subprocesses intentionally share the real per-user runtime lock.
// Serialize only tests that launch a wizard; read-only menu tests stay parallel.
static SETUP_PROCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

fn command(path: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_piair"));
    command.arg("--config").arg(path).arg("setup");
    command
}
#[test]
fn terminal_start_invalid_input_cancel_eof_and_sigint_preserve_config() {
    let _setup = SETUP_PROCESS.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let original = "# unchanged\n";
    fs::write(&path, original).unwrap();
    for mode in ["q", "eof", "sigint"] {
        let mut terminal = Terminal::start(command(&path));
        terminal.until("Choose input");
        terminal.answer("invalid\n");
        terminal.until("Enter a number");
        match mode {
            "q" => terminal.answer("q\n"),
            "eof" => terminal.answer("\x04"),
            _ => {
                assert!(Command::new("kill")
                    .args(["-INT", &terminal.child.id().to_string()])
                    .status()
                    .unwrap()
                    .success());
            }
        }
        terminal.finish(true);
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
#[test]
fn setup_rejects_redirected_streams() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("config.toml");
    fs::write(&path, "").unwrap();
    let output = command(&path).stdin(Stdio::null()).output().unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("requires interactive stdin and stdout")
    );
    let master =
        rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)
            .unwrap();
    rustix::pty::grantpt(&master).unwrap();
    rustix::pty::unlockpt(&master).unwrap();
    let name = rustix::pty::ptsname(&master, Vec::new()).unwrap();
    let slave = File::options()
        .read(true)
        .write(true)
        .open(std::ffi::OsStr::from_bytes(name.to_bytes()))
        .unwrap();
    let output = command(&path).stdin(slave).output().unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("requires interactive stdin and stdout")
    );
}

#[test]
fn daily_menu_missing_config_retries_returns_and_cancels_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "# unchanged\n").unwrap();
    for mode in ["q", "eof", "sigint"] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_piair"));
        command.arg("--config").arg(&path);
        let mut terminal = Terminal::start(command);
        terminal.until("1. Run piair setup");
        terminal.answer("invalid\n");
        terminal.until("Enter a number");
        terminal.output.clear();
        terminal.answer("2\n");
        // A freshly rendered menu establishes return from the status action.
        terminal.until("Playback configuration needs setup.");
        terminal.until("Choose:");
        match mode {
            "q" => terminal.answer("q\n"),
            "eof" => terminal.answer("\x04"),
            _ => {
                assert!(Command::new("kill")
                    .args(["-INT", &terminal.child.id().to_string()])
                    .status()
                    .unwrap()
                    .success());
            }
        }
        terminal.finish(true);
        assert_eq!(fs::read_to_string(&path).unwrap(), "# unchanged\n");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn daily_existing_configuration_shows_saved_values_and_returns_from_settings() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let source = "receiver_allowlist=['device:aabbccddeeff']\n[playback]\nreceiver_id='device:aabbccddeeff'\ncapture_device='synthetic-only'\nauthentication='own-tone-transient'\n";
    fs::write(&path, source).unwrap();
    let state = dir.path().join("receiver-state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let selected = state.join("selected.toml");
    let saved = "display_name='Synthetic Living Room'\nid='device:aabbccddeeff'\nmethod='transient'\naddresses=['127.0.0.1']\nport=9\n[capabilities]\nfeatures='0001420000080200'\nflags=0\n";
    fs::write(&selected, saved).unwrap();
    fs::set_permissions(&selected, fs::Permissions::from_mode(0o600)).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_piair"));
    command.arg("--config").arg(&path);
    let mut terminal = Terminal::start(command);
    terminal.until("Q. Exit");
    for choice in [
        "1. Status",
        "2. Service controls unavailable",
        "3. Choose speaker",
        "4. Test audio (49 seconds)",
        "5. Settings",
        "6. Logs",
        "Q. Exit",
    ] {
        assert!(terminal.output.contains(choice), "{}", terminal.output);
    }
    assert!(!terminal.output.contains("2. Stop PiAir"));
    assert!(terminal.output.contains("Synthetic Living Room"));
    assert!(terminal.output.contains("Audio input: synthetic-only"));
    assert!(terminal.output.contains("Complete"));
    terminal.output.clear();
    terminal.answer("2\n");
    terminal.until("Service control is unavailable");
    terminal.until("6. Logs");
    terminal.answer("5\n");
    terminal.until("2. View saved playback settings");
    terminal.output.clear();
    terminal.answer("q\n");
    terminal.until("6. Logs");
    terminal.answer("q\n");
    terminal.finish(true);
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
    assert_eq!(fs::read_to_string(&selected).unwrap(), saved);
}

#[test]
fn absent_default_configuration_can_launch_existing_setup_and_return() {
    let _setup = SETUP_PROCESS.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_piair"));
    command
        .env_remove("PIAIR_CONFIG")
        .env_remove("XDG_CONFIG_HOME")
        .env("HOME", dir.path());
    let mut terminal = Terminal::start(command);
    terminal.until("1. Run piair setup");
    terminal.answer("1\n");
    terminal.until("Welcome to PiAir setup.");
    terminal.until("Choose input");
    terminal.output.clear();
    terminal.answer("q\n");
    terminal.until("Playback configuration needs setup.");
    terminal.answer("q\n");
    terminal.finish(true);
    assert!(!dir.path().join(".config/piair/config.toml").exists());
}

#[test]
fn interrupting_menu_forwards_cancellation_to_setup_child_and_releases_lock() {
    let _setup = SETUP_PROCESS.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_piair"));
    command
        .env_remove("PIAIR_CONFIG")
        .env_remove("XDG_CONFIG_HOME")
        .env("HOME", dir.path());
    let mut terminal = Terminal::start(command);
    terminal.until("1. Run piair setup");
    terminal.answer("1\n");
    terminal.until("Choose input");
    // Signal the menu only: it must interrupt/reap its child rather than wait
    // indefinitely for the child's next prompt, or start any service.
    assert!(Command::new("kill")
        .args(["-INT", &terminal.child.id().to_string()])
        .status()
        .unwrap()
        .success());
    terminal.finish(true);
    let path = dir.path().join("check.toml");
    fs::write(&path, "").unwrap();
    let mut next_command = Command::new(env!("CARGO_BIN_EXE_piair"));
    next_command.arg("--config").arg(&path).arg("setup");
    let mut next = Terminal::start(next_command);
    next.until("Choose input");
    next.answer("q\n");
    next.finish(true);
}
