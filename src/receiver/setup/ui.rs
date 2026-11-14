//! Line-oriented presentation with injectable input. EOF always cancels.
use anyhow::{ensure, Result};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug)]
pub(super) struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("setup cancelled")
    }
}
impl std::error::Error for Cancelled {}

pub(super) struct Ui<R, W> {
    pub read: R,
    pub out: W,
}
impl<R: FnMut() -> Result<String>, W: Write> Ui<R, W> {
    pub fn say(&mut self, text: impl std::fmt::Display) -> Result<()> {
        writeln!(self.out, "{text}")?;
        self.out.flush()?;
        Ok(())
    }
    pub fn line(&mut self, prompt: &str) -> Result<String> {
        self.say(prompt)?;
        let line = (self.read)()?;
        if !line.ends_with('\n') || line.trim().eq_ignore_ascii_case("q") {
            return Err(Cancelled.into());
        }
        Ok(line.trim().to_owned())
    }
    pub fn choice(&mut self, prompt: &str, count: usize, default: Option<usize>) -> Result<usize> {
        loop {
            let line = self.line(prompt)?;
            if line.is_empty() {
                if let Some(default) = default {
                    return Ok(default);
                }
            }
            if let Ok(n) = line.parse::<usize>() {
                if (1..=count).contains(&n) {
                    return Ok(n);
                }
            }
            self.say(format!("Enter a number from 1 to {count}, or q to cancel."))?;
        }
    }
    pub fn confirm(&mut self, prompt: &str) -> Result<bool> {
        Ok(self.choice(&format!("{prompt}\n1. Yes\n2. No [default]"), 2, Some(2))? == 1)
    }
    pub fn secret(&mut self, prompt: &str) -> Result<zeroize::Zeroizing<String>> {
        use rustix::termios::{tcgetattr, tcsetattr, LocalModes, OptionalActions, Termios};
        use std::io::IsTerminal;
        struct Echo(Option<Termios>);
        impl Drop for Echo {
            fn drop(&mut self) {
                if let Some(old) = &self.0 {
                    let _ = tcsetattr(io::stdin(), OptionalActions::Now, old);
                }
            }
        }
        let mut guard = Echo(None);
        if io::stdin().is_terminal() {
            let old = tcgetattr(io::stdin())?;
            let mut hidden = old.clone();
            hidden.local_modes.remove(LocalModes::ECHO);
            tcsetattr(io::stdin(), OptionalActions::Now, &hidden)?;
            guard.0 = Some(old);
        }
        let answer = self.line(prompt).map(zeroize::Zeroizing::new);
        drop(guard);
        self.say("")?;
        answer
    }
    pub fn number<T: std::str::FromStr>(&mut self, prompt: &str) -> Result<T> {
        loop {
            if let Ok(n) = self.line(prompt)?.parse() {
                return Ok(n);
            }
            self.say("Enter a valid number, or q to cancel.")?;
        }
    }
}

pub(super) fn require_terminal(stdin: bool, stdout: bool) -> Result<()> {
    ensure!(stdin && stdout, "piair setup requires interactive stdin and stdout; run it directly in a terminal or SSH with a TTY");
    Ok(())
}

pub(super) fn terminal_line(cancel: &AtomicBool) -> Result<String> {
    use rustix::event::{poll, PollFd, PollFlags, Timespec};
    let stdin = io::stdin();
    let mut line = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        match poll(
            &mut fds,
            Some(&Timespec {
                tv_sec: 0,
                tv_nsec: 100_000_000,
            }),
        ) {
            Ok(0) => continue,
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Err(e.into()),
            _ => {}
        }
        // Read one byte directly from the terminal fd. std::io::Stdin's shared
        // buffered reader can hide pasted q/EOF behind a later poll, preventing
        // the service-backed test from observing cancellation promptly.
        let mut byte = [0u8; 1];
        match rustix::io::read(&stdin, &mut byte) {
            Ok(0) => return Ok(String::from_utf8(line)?),
            Ok(_) => {
                line.push(byte[0]);
                if byte[0] == b'\n' {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(Cancelled.into());
                    }
                    return Ok(String::from_utf8(line)?);
                }
            }
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    #[test]
    fn tty_gate_and_numbered_retry() {
        for (a, b) in [(false, false), (true, false), (false, true)] {
            assert!(require_terminal(a, b).is_err());
        }
        require_terminal(true, true).unwrap();
        let mut input = "bad\n0\n4\n2\n".as_bytes();
        let mut ui = Ui {
            read: || {
                let mut s = String::new();
                input.read_line(&mut s)?;
                Ok(s)
            },
            out: Vec::new(),
        };
        assert_eq!(ui.choice("Choose", 3, None).unwrap(), 2);
        assert!(String::from_utf8(ui.out)
            .unwrap()
            .contains("Enter a number"));
    }
    #[test]
    fn eof_partial_line_and_cancel_never_confirm() {
        for line in ["", "1", "q\n", "Q\n"] {
            let mut ui = Ui {
                read: || Ok(line.to_owned()),
                out: Vec::new(),
            };
            assert!(ui.confirm("Proceed?").is_err());
        }
        for (line, yes) in [("\n", false), ("2\n", false), ("1\n", true)] {
            let mut ui = Ui {
                read: || Ok(line.to_owned()),
                out: Vec::new(),
            };
            assert_eq!(ui.confirm("Proceed?").unwrap(), yes);
        }
    }
}
