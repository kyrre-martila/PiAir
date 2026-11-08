//! Terminal presentation and orchestration; transport authorization stays in Wire.
use super::{compatibility::Method, discovery, exact_id, pair, select, store::Store, AuthProfile};
use crate::{config::Loaded, discovery::Receiver};
use anyhow::{bail, ensure, Context, Result};
use std::io::{self, BufRead, IsTerminal, Write};

fn terminal() -> Result<()> {
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "requires an interactive terminal"
    );
    Ok(())
}

pub(super) fn confirm(
    input: &mut impl BufRead,
    output: &mut impl Write,
    prompt: &str,
) -> Result<()> {
    write!(output, "{prompt}")?;
    output.flush()?;
    let mut line = String::new();
    let read = input.read_line(&mut line)?;
    ensure!(
        read > 0
            && line.ends_with('\n')
            && matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
        "receiver operation cancelled"
    );
    Ok(())
}

pub(super) fn confirm_terminal(prompt: &str) -> Result<()> {
    terminal()?;
    confirm(&mut io::stdin().lock(), &mut io::stdout().lock(), prompt)
}

fn ordered(receivers: &[Receiver]) -> Vec<&Receiver> {
    let mut rows: Vec<_> = receivers.iter().collect();
    rows.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
    rows
}

fn listing(loaded: &Loaded, receivers: &[Receiver], output: &mut impl Write) -> Result<()> {
    let store = Store::new(&loaded.path)?;
    let selected = store.selection()?;
    writeln!(output, "Available AirPlay receivers (unverified advertisements; discovery does not prove identity):")?;
    for (index, receiver) in ordered(receivers).iter().enumerate() {
        let approved = loaded.config.allows(&receiver.id);
        let paired = if exact_id(&receiver.id).is_ok() {
            store.credential(&receiver.id)?.is_some()
        } else {
            false
        };
        writeln!(
            output,
            "{}. {:?} [{}; {}; {}]",
            index + 1,
            receiver.name,
            if approved {
                "Approved"
            } else {
                "Not approved; ineligible"
            },
            if selected.as_ref().is_some_and(|s| s.id == receiver.id) {
                "Selected"
            } else {
                "Not selected"
            },
            if paired {
                "Paired locally; not authenticated"
            } else {
                "Not paired; authentication required"
            }
        )?;
        writeln!(output, "   Device ID: {}", receiver.id)?;
        for service in &receiver.services {
            writeln!(
                output,
                "   Address: {:?}:{} ({:?})",
                service.addresses, service.port, service.service_type
            )?;
        }
    }
    Ok(())
}

fn choice(input: &mut impl BufRead, output: &mut impl Write, count: usize) -> Result<usize> {
    write!(output, "Select receiver (or q to cancel): ")?;
    output.flush()?;
    let mut line = String::new();
    let n = input.read_line(&mut line)?;
    ensure!(
        n != 0 && line.ends_with('\n') && !line.trim().eq_ignore_ascii_case("q"),
        "receiver selection cancelled"
    );
    let index = line
        .trim()
        .parse::<usize>()
        .context("invalid receiver selection")?;
    ensure!((1..=count).contains(&index), "invalid receiver selection");
    Ok(index - 1)
}

fn choose(loaded: &Loaded) -> Result<String> {
    terminal()?;
    let receivers = discovery::discover(loaded.config.discovery_timeout_secs)?;
    listing(loaded, &receivers, &mut io::stdout().lock())?;
    ensure!(!receivers.is_empty(), "no receivers discovered");
    let index = choice(
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        receivers.len(),
    )?;
    let receiver = ordered(&receivers)[index];
    describe(&receivers, &receiver.id)?;
    require_approval(loaded, &receiver.id)?;
    Ok(receiver.id.clone())
}

fn require_approval(loaded: &Loaded, id: &str) -> Result<()> {
    exact_id(id)?;
    ensure!(loaded.config.allows(id), "receiver unapproved. Verify its identity independently, then manually add {id:?} to receiver_allowlist in {} and run piair --config {:?} config check. Configuration is not edited by this wizard", loaded.path.display(), loaded.path);
    Ok(())
}

pub(super) fn describe(receivers: &[Receiver], id: &str) -> Result<()> {
    let matches: Vec<_> = receivers.iter().filter(|r| r.id == id).collect();
    ensure!(
        matches.len() == 1,
        "receiver missing or ambiguous in discovery"
    );
    let receiver = matches[0];
    println!("Receiver: {:?}\nDevice ID: {}", receiver.name, receiver.id);
    for service in &receiver.services {
        println!("Address: {:?}:{}", service.addresses, service.port);
    }
    Ok(())
}

pub(super) fn already_paired(loaded: &Loaded, id: &str) -> Result<bool> {
    require_approval(loaded, id)?;
    Ok(Store::new(&loaded.path)?.credential(id)?.is_some())
}

pub(crate) fn pairing(loaded: &Loaded) -> Result<()> {
    let id = choose(loaded)?;
    if already_paired(loaded, &id)? {
        println!("Already paired locally; no connection made or credentials replaced. Use authenticate explicitly to verify the receiver now.");
        return Ok(());
    }
    let selected = Store::new(&loaded.path)?.selection()?;
    if !selected.as_ref().is_some_and(|s| s.id == id) {
        println!("Choose the persistent pairing method explicitly: 1. PIN displayed by receiver; 2. Existing HAP setup code. No protocol fallback.");
        let method = match choice(&mut io::stdin().lock(), &mut io::stdout().lock(), 2)? {
            0 => Method::PinDisplay,
            1 => Method::SetupCode,
            _ => unreachable!(),
        };
        select(loaded, &id, method)?;
    } else if selected
        .as_ref()
        .is_some_and(|s| s.method == Method::Transient)
    {
        bail!("selected transient method does not enroll credentials; explicitly select pin-display or setup-code first");
    }
    pair(loaded, &id, false, false, AuthProfile::Legacy)
}

pub(crate) fn choose_for_stream(loaded: &Loaded) -> Result<String> {
    let id = choose(loaded)?;
    let selected = Store::new(&loaded.path)?.selection()?;
    if !selected.as_ref().is_some_and(|s| s.id == id) {
        println!("Choose an explicit method: 1. PIN-display persistent credentials; 2. Setup-code persistent credentials; 3. Transient SRP (no persistent identity verification). No fallback.");
        let method = match choice(&mut io::stdin().lock(), &mut io::stdout().lock(), 3)? {
            0 => Method::PinDisplay,
            1 => Method::SetupCode,
            2 => Method::Transient,
            _ => unreachable!(),
        };
        select(loaded, &id, method)?;
    }
    // An existing selection remains pinned; Wire revalidates before connecting.
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmation_is_explicit_and_eof_denies() {
        for input in ["y\n", "Y\n", "yes\n", "YeS\n"] {
            assert!(confirm(&mut input.as_bytes(), &mut Vec::new(), "Continue? [y/N]: ").is_ok());
        }
        for input in [
            "",
            "\n",
            "n\n",
            "no\n",
            "maybe\n",
            "device:aabbccddeeff\n",
            "yes please\n",
            "y",
            "yes",
        ] {
            assert!(confirm(&mut input.as_bytes(), &mut Vec::new(), "").is_err());
        }
    }
    #[test]
    fn selections_reject_invalid_and_cancelled_input() {
        for input in ["", "1", "q\n", "Q\n", "\n", "0\n", "3\n", "-1\n", "yes\n"] {
            assert!(choice(&mut input.as_bytes(), &mut Vec::new(), 2).is_err());
        }
        assert_eq!(
            choice(&mut "2\n".as_bytes(), &mut Vec::new(), 2).unwrap(),
            1
        );
    }
    #[test]
    fn duplicate_names_have_stable_id_order_and_explicit_states() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded = Loaded {
            config: crate::config::Config::default(),
            path: tmp.path().join("config.toml"),
            source: "test",
        };
        let receivers = vec![
            Receiver {
                name: "Room".into(),
                id: "device:bb1122334455".into(),
                services: vec![],
            },
            Receiver {
                name: "Room".into(),
                id: "device:aa1122334455".into(),
                services: vec![],
            },
        ];
        let mut output = Vec::new();
        listing(&loaded, &receivers, &mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.find("device:aa").unwrap() < output.find("device:bb").unwrap());
        assert!(output.contains("Not approved; ineligible"));
        assert!(!output.contains("Ready"));
        assert!(require_approval(&loaded, &receivers[0].id).is_err());
    }
}
