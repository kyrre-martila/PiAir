//! Terminal presentation and orchestration; transport authorization stays in Wire.
use super::{compatibility::Method, discovery, exact_id, store::Store, AuthProfile, Selection};
use crate::{config::Loaded, discovery::Receiver};
use anyhow::{ensure, Context, Result};
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

pub(super) struct ProposedSelection {
    pub selected: Selection,
    previous: Option<Selection>,
    enrollment: bool,
}

pub(super) fn confirm_operation(
    loaded: &Loaded,
    proposed: Option<&ProposedSelection>,
    snapshot: &[Receiver],
    prompt: &str,
) -> Result<()> {
    terminal()?;
    authorize(
        loaded,
        proposed,
        snapshot,
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        prompt,
    )
}

fn authorize(
    loaded: &Loaded,
    proposed: Option<&ProposedSelection>,
    snapshot: &[Receiver],
    input: &mut impl BufRead,
    output: &mut impl Write,
    prompt: &str,
) -> Result<()> {
    if let Some(proposed) = proposed {
        writeln!(output, "Confirming saves {} with method {:?} and its pinned endpoint as the active receiver selection. It remains selected if the subsequent operation fails.", proposed.selected.id, proposed.selected.method)?;
    }
    confirm(input, output, prompt)?;
    if let Some(proposed) = proposed {
        // Revalidate current approval, observed endpoint/capabilities and credential
        // binding after confirmation, before persisting or opening a socket.
        let current = crate::config::load(Some(loaded.path.clone()))?;
        let observed = super::validate_selection(
            &current,
            &proposed.selected.id,
            proposed.selected.method,
            snapshot,
        )?;
        ensure!(
            proposed.selected.matches_observation(&observed),
            "receiver changed; review and select again explicitly"
        );
        let store = Store::new(&loaded.path)?;
        if proposed.enrollment {
            ensure!(
                store.credential(&proposed.selected.id)?.is_none(),
                "credentials now exist; use authenticate explicitly; no re-enrollment"
            );
        } else if proposed.selected.method != Method::Transient {
            ensure!(
                store.credential(&proposed.selected.id)?.is_some(),
                "pair this receiver explicitly first"
            );
        }
        store.select_if_unchanged(&proposed.selected, proposed.previous.as_ref())?;
    }
    Ok(())
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

fn choose(
    loaded: &Loaded,
    receivers: &[Receiver],
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<String> {
    listing(loaded, receivers, output)?;
    ensure!(!receivers.is_empty(), "no receivers discovered");
    let index = choice(input, output, receivers.len())?;
    let receiver = ordered(receivers)[index];
    describe(receivers, &receiver.id)?;
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
    terminal()?;
    let snapshot = discovery::discover(loaded.config.discovery_timeout_secs)?;
    let proposed = prepare(
        loaded,
        &snapshot,
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        true,
    )?;
    if let Some(proposed) = proposed {
        let id = proposed.selected.id.clone();
        super::pair_with_selection(
            loaded,
            &id,
            false,
            false,
            AuthProfile::Legacy,
            Some(proposed),
        )?;
    }
    Ok(())
}

pub(super) fn choose_for_stream(loaded: &Loaded) -> Result<ProposedSelection> {
    terminal()?;
    let snapshot = discovery::discover(loaded.config.discovery_timeout_secs)?;
    prepare(
        loaded,
        &snapshot,
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        false,
    )?
    .context("no playback selection")
}

// Injectable presentation seam; all choices remain provisional, including method.
fn prepare(
    loaded: &Loaded,
    snapshot: &[Receiver],
    input: &mut impl BufRead,
    output: &mut impl Write,
    enrollment: bool,
) -> Result<Option<ProposedSelection>> {
    let previous = Store::new(&loaded.path)?.selection()?;
    let id = choose(loaded, snapshot, input, output)?;
    if enrollment && already_paired(loaded, &id)? {
        writeln!(output, "Stored credentials exist for {id}; this does not establish current authentication. Active receiver selection is unchanged. To use this pairing, explicitly select this receiver with its documented persistent --method, then authenticate. If endpoint or credential binding checks fail, investigate before any explicit unpair/re-enrollment.")?;
        return Ok(None);
    }
    let selected = if let Some(selected) = previous.as_ref().filter(|s| s.id == id) {
        ensure!(!enrollment || selected.method != Method::Transient,
            "selected transient method does not enroll credentials; explicitly select pin-display or setup-code first");
        let observed = super::validate_selection(loaded, &id, selected.method, snapshot)?;
        ensure!(
            selected.matches_observation(&observed),
            "pinned receiver changed; review and select again explicitly"
        );
        selected.clone()
    } else {
        writeln!(output, "Choose an explicit method: 1. PIN-display; 2. Existing HAP setup code{}; no fallback. Selection is provisional until final confirmation.", if enrollment { "" } else { "; 3. Transient SRP (no persistent identity verification)" })?;
        let method = match choice(input, output, if enrollment { 2 } else { 3 })? {
            0 => Method::PinDisplay,
            1 => Method::SetupCode,
            2 => Method::Transient,
            _ => unreachable!(),
        };
        super::validate_selection(loaded, &id, method, snapshot)?
    };
    Ok(Some(ProposedSelection {
        selected,
        previous,
        enrollment,
    }))
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

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::receiver::{compatibility::Capabilities, store::Credential};
    use std::{collections::BTreeMap, fs};
    const OLD: &str = "device:aabbccddeeff";
    const NEW: &str = "device:112233445566";
    const PI: &str = "12345678-1234-1234-1234-123456789ABC";

    struct Fixture {
        _dir: tempfile::TempDir,
        loaded: Loaded,
        snapshot: Vec<Receiver>,
    }
    fn credential(id: &str) -> Credential {
        Credential {
            version: 2,
            receiver_id: id.into(),
            pairing_id: PI.into(),
            advertised_pi: Some(PI.into()),
            controller_id: "00000000-0000-4000-8000-000000000001".into(),
            seed: [11; 32],
            receiver_key: ed25519_dalek::SigningKey::from_bytes(&[22; 32])
                .verifying_key()
                .to_bytes(),
        }
    }
    impl Fixture {
        fn new(paired: bool) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            fs::write(&path, format!("receiver_allowlist=['{OLD}', '{NEW}']")).unwrap();
            let loaded = crate::config::load(Some(path)).unwrap();
            let store = Store::new(&loaded.path).unwrap();
            store
                .select(&Selection {
                    id: OLD.into(),
                    advertised_pi: Some(PI.into()),
                    method: Method::SetupCode,
                    addresses: ["127.0.0.1".parse().unwrap()].into(),
                    port: 1234,
                    capabilities: Capabilities {
                        features: 1 << 46,
                        flags: None,
                        password: None,
                        access_control: None,
                        access_control_type: None,
                        protocol_version: None,
                    },
                })
                .unwrap();
            store.save(&credential(OLD)).unwrap();
            if paired {
                store.save(&credential(NEW)).unwrap();
            }
            let info = mdns_sd::ServiceInfo::new(
                "_airplay._tcp.local.",
                "New receiver",
                "new.local.",
                "127.0.0.2",
                7000,
                [
                    ("deviceid", "11:22:33:44:55:66"),
                    ("pi", PI),
                    ("features", "0x80200,0x14000"),
                    ("sf", "0x0"),
                ]
                .as_slice(),
            )
            .unwrap();
            Self {
                _dir: dir,
                loaded,
                snapshot: vec![Receiver {
                    id: NEW.into(),
                    name: "New receiver".into(),
                    services: vec![discovery::Advertisement::test_from_info(&info)],
                }],
            }
        }
        fn files(&self) -> BTreeMap<String, Vec<u8>> {
            let mut files = BTreeMap::new();
            files.insert("config.toml".into(), fs::read(&self.loaded.path).unwrap());
            for entry in
                fs::read_dir(self.loaded.path.parent().unwrap().join("receiver-state")).unwrap()
            {
                let entry = entry.unwrap();
                files.insert(
                    entry.file_name().to_string_lossy().into_owned(),
                    fs::read(entry.path()).unwrap(),
                );
            }
            files
        }
        fn propose(&self, input: &str, enrollment: bool) -> Result<Option<ProposedSelection>> {
            prepare(
                &self.loaded,
                &self.snapshot,
                &mut input.as_bytes(),
                &mut Vec::new(),
                enrollment,
            )
        }
        fn authorize(&self, proposed: &ProposedSelection, input: &str) -> Result<()> {
            authorize(
                &self.loaded,
                Some(proposed),
                &self.snapshot,
                &mut input.as_bytes(),
                &mut Vec::new(),
                "Continue? [y/N]: ",
            )
        }
    }
    #[test]
    fn cancelled_pairing_and_playback_leave_all_persisted_bytes_unchanged() {
        for enrollment in [true, false] {
            let f = Fixture::new(!enrollment);
            let before = f.files();
            // Cancel at receiver or method choice, including EOF after receiver choice.
            for input in ["q\n", "", "\n", "1\nq\n", "1\n", "1\n\n"] {
                assert!(f.propose(input, enrollment).is_err(), "{input:?}");
                assert_eq!(f.files(), before);
            }
            for method in ["1", "2"].into_iter().chain((!enrollment).then_some("3")) {
                let proposed = f
                    .propose(&format!("1\n{method}\n"), enrollment)
                    .unwrap()
                    .unwrap();
                assert_eq!(f.files(), before); // receiver AND method are still provisional
                assert!(proposed.selected.id == NEW);
                for answer in ["\n", "n\n", "no\n", "q\n", "", "yes", "invalid\n"] {
                    assert!(f.authorize(&proposed, answer).is_err(), "{answer:?}");
                    assert_eq!(f.files(), before); // endpoint, method, credentials and allowlist
                }
            }
        }
    }
    #[test]
    fn already_paired_does_not_select_or_authenticate() {
        let f = Fixture::new(true);
        let before = f.files();
        let mut output = Vec::new();
        assert!(prepare(
            &f.loaded,
            &f.snapshot,
            &mut "1\n".as_bytes(),
            &mut output,
            true
        )
        .unwrap()
        .is_none());
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("does not establish current authentication"));
        assert!(output.contains("explicitly select this receiver"));
        assert_eq!(f.files(), before);
    }
    #[test]
    fn confirmed_selection_commits_once_and_wire_accepts_its_authorization() {
        for enrollment in [true, false] {
            let mut f = Fixture::new(!enrollment);
            let listener = std::net::TcpListener::bind("127.0.0.2:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            f.snapshot[0].services[0].port = listener.local_addr().unwrap().port();
            let proposed = f.propose("1\n2\n", enrollment).unwrap().unwrap();
            let before = f.files();
            f.authorize(&proposed, "YeS\n").unwrap();
            let store = Store::new(&f.loaded.path).unwrap();
            assert!(store.selection().unwrap().as_ref() == Some(&proposed.selected));
            let mut after = f.files();
            after.insert("selected.toml".into(), before["selected.toml"].clone());
            assert_eq!(before, after); // no credential/config writes at confirmation
            let wire = crate::receiver::wire::Wire::connect(
                &f.loaded.path,
                NEW,
                &proposed.selected,
                &f.snapshot,
            )
            .unwrap();
            drop(wire);
            assert!(listener.accept().is_ok());
            // A post-confirmation failure leaves the authorized selection in place;
            // there is no restoration path that could overwrite newer user choices.
            assert!(store.selection().unwrap().as_ref() == Some(&proposed.selected));
        }
    }
    #[test]
    fn confirmation_rechecks_concurrent_selection_approval_endpoint_and_credentials() {
        for fault in [
            "selection",
            "approval",
            "endpoint",
            "credential",
            "missing-credential",
        ] {
            let mut f = Fixture::new(fault == "missing-credential");
            let proposed = f
                .propose("1\n1\n", fault != "missing-credential")
                .unwrap()
                .unwrap();
            let store = Store::new(&f.loaded.path).unwrap();
            match fault {
                "selection" => {
                    let mut newer = store.selection().unwrap().unwrap();
                    newer.port = 4321;
                    store.select(&newer).unwrap();
                }
                "approval" => fs::write(&f.loaded.path, "receiver_allowlist=[]").unwrap(),
                "endpoint" => f.snapshot[0].services[0].port += 1,
                "credential" => store.save(&credential(NEW)).unwrap(),
                "missing-credential" => store.remove(NEW).unwrap(),
                _ => unreachable!(),
            }
            let before = f.files();
            assert!(f.authorize(&proposed, "y\n").is_err(), "{fault}");
            assert_eq!(f.files(), before);
        }
    }
    #[test]
    fn explicit_select_still_persists_immediately_without_confirmation() {
        let f = Fixture::new(false);
        super::super::select_from_snapshot(&f.loaded, NEW, Method::SetupCode, &f.snapshot).unwrap();
        let selected = Store::new(&f.loaded.path)
            .unwrap()
            .selection()
            .unwrap()
            .unwrap();
        assert!(
            selected.id == NEW && selected.method == Method::SetupCode && selected.port == 7000
        );
    }
    #[test]
    fn selection_writers_fail_closed_while_another_writer_holds_directory_lock() {
        let f = Fixture::new(false);
        let proposed = f.propose("1\n1\n", true).unwrap().unwrap();
        let lock = fs::File::open(f.loaded.path.parent().unwrap().join("receiver-state")).unwrap();
        lock.try_lock().unwrap();
        let before = f.files();
        assert!(f.authorize(&proposed, "y\n").is_err());
        assert!(Store::new(&f.loaded.path)
            .unwrap()
            .select(&proposed.selected)
            .is_err());
        assert_eq!(f.files(), before);
        drop(lock);
        f.authorize(&proposed, "y\n").unwrap();
    }
    #[test]
    fn absent_selection_is_not_created_on_cancellation() {
        let f = Fixture::new(false);
        fs::remove_file(
            f.loaded
                .path
                .parent()
                .unwrap()
                .join("receiver-state/selected.toml"),
        )
        .unwrap();
        let before = f.files();
        let proposed = f.propose("1\n1\n", true).unwrap().unwrap();
        assert!(proposed.previous.is_none());
        assert!(f.authorize(&proposed, "n\n").is_err());
        assert_eq!(f.files(), before);
        f.authorize(&proposed, "y\n").unwrap();
        assert!(
            Store::new(&f.loaded.path)
                .unwrap()
                .selection()
                .unwrap()
                .as_ref()
                == Some(&proposed.selected)
        );
    }
    #[test]
    fn existing_credential_binding_mismatch_fails_without_selection_changes() {
        let mut f = Fixture::new(true);
        f.snapshot[0].services[0]
            .capabilities
            .insert("pi".into(), "Changed".into());
        let before = f.files();
        assert!(f.propose("1\n1\n", false).is_err());
        assert_eq!(f.files(), before);
    }
}
