use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::ffi::OsStrExt,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
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
