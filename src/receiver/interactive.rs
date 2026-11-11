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

#[derive(Clone, Copy)]
struct ReceiverState {
    approved: bool,
    selected: bool,
    paired: bool,
}

impl ReceiverState {
    fn label(self) -> &'static str {
        match (self.approved, self.selected, self.paired) {
            (false, _, _) => "Not approved",
            (true, true, true) => "Paired, selected",
            (true, false, true) => "Paired",
            (true, true, false) => "Selected",
            (true, false, false) => "Not paired",
        }
    }
}

// Advertised names are untrusted terminal text, never receiver identities.
fn display_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn short_name(name: &str) -> String {
    // Leave room for numbering and the longest state label on narrow terminals.
    let limit = std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(80)
        .saturating_sub(28)
        .clamp(8, 32);
    let name = display_name(name);
    if name.chars().count() > limit {
        name.chars().take(limit - 1).chain(['…']).collect()
    } else {
        name
    }
}

fn listing(loaded: &Loaded, receivers: &[Receiver], output: &mut impl Write) -> Result<()> {
    render_listing(loaded, receivers, output, false)
}

pub(super) fn render_listing(
    loaded: &Loaded,
    receivers: &[Receiver],
    output: &mut impl Write,
    detailed: bool,
) -> Result<()> {
    let store = Store::new(&loaded.path)?;
    let selected = store.selection()?;
    writeln!(output, "Available AirPlay receivers:\n")?;
    if receivers.is_empty() {
        writeln!(
            output,
            "No receivers discovered. Check the local network and try again."
        )?;
    }
    let width = receivers
        .iter()
        .map(|r| short_name(&r.name).chars().count())
        .max()
        .unwrap_or(0);
    for (index, receiver) in ordered(receivers).iter().enumerate() {
        let state = ReceiverState {
            approved: loaded.config.allows(&receiver.id),
            selected: selected.as_ref().is_some_and(|s| s.id == receiver.id),
            paired: exact_id(&receiver.id).is_ok() && store.credential(&receiver.id)?.is_some(),
        };
        let name = short_name(&receiver.name);
        let duplicate = receivers
            .iter()
            .filter(|r| short_name(&r.name) == short_name(&receiver.name))
            .count()
            > 1;
        write!(
            output,
            "  {}. {}{}  [{}]",
            index + 1,
            name,
            " ".repeat(width.saturating_sub(name.chars().count())),
            state.label()
        )?;
        if duplicate && !detailed {
            write!(output, " ({})", receiver.id)?;
        }
        writeln!(output)?;
        if detailed {
            writeln!(
                output,
                "     Receiver: {}\n     Device ID: {}",
                display_name(&receiver.name),
                receiver.id
            )?;
            writeln!(
                output,
                "     Approved: {}\n     Selected: {}\n     Paired locally: {}",
                state.approved, state.selected, state.paired
            )?;
            for service in &receiver.services {
                writeln!(
                    output,
                    "     Service: {} port {}\n     Addresses (IPv4/IPv6): {:?}",
                    service.service_type, service.port, service.addresses
                )?;
            }
        }
    }
    Ok(())
}

fn choice(input: &mut impl BufRead, output: &mut impl Write, count: usize) -> Result<usize> {
    numbered_choice(input, output, count, "receiver")
}

fn numbered_choice(
    input: &mut impl BufRead,
    output: &mut impl Write,
    count: usize,
    subject: &str,
) -> Result<usize> {
    write!(output, "Select {subject} (1-{count}, q to cancel): ")?;
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
    println!(
        "Receiver: {}\nDevice ID: {}",
        display_name(&receiver.name),
        receiver.id
    );
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

// The user must supply evidence advertisements cannot establish.
fn enrollment_prerequisite(
    method: Method,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<()> {
    writeln!(output, "Persistent pairing enrolls permanent verified credentials; it is separate from session authentication and playback.")?;
    let prompt = match method {
        Method::PinDisplay => "PIN-display requires a receiver-supported PIN display or equivalent independently verified PIN delivery mechanism. Is that available for this receiver? [y/N]: ",
        Method::SetupCode => "Existing HAP setup-code enrollment requires a genuine, independently obtained HAP setup code for this receiver. An AirPlay password is not a HAP setup code. Do you have that code? [y/N]: ",
        Method::StoredHap => anyhow::bail!("stored HAP is authentication-only; no enrollment"),
        Method::Transient => anyhow::bail!("transient authentication does not enroll persistent credentials; use authenticate explicitly; no fallback"),
    };
    writeln!(output, "Do not invent codes or guess methods. If neither prerequisite is available, persistent enrollment cannot currently proceed through these methods. Cancel safely; active selection and credentials remain unchanged. For a separately authorized session-only diagnostic, see docs/RECEIVER_AUTHENTICATION.md and the explicit authenticate --experimental-owntone-transient command. No automatic fallback.")?;
    confirm(input, output, prompt)
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
            "selected transient method does not enroll credentials; persistent enrollment requires verified PIN delivery or a genuine independently obtained HAP setup code. If neither is available, enrollment cannot currently proceed; see docs/RECEIVER_AUTHENTICATION.md for explicitly authorized session authentication");
        let observed = super::validate_selection(loaded, &id, selected.method, snapshot)?;
        ensure!(
            selected.matches_observation(&observed),
            "pinned receiver changed; review and select again explicitly"
        );
        selected.clone()
    } else {
        writeln!(output, "Choose an explicit method: 1. PIN-display (requires supported PIN delivery); 2. Existing HAP setup code (requires genuine independently obtained code, not an AirPlay password){}; no fallback. Selection is provisional until final confirmation.", if enrollment { "" } else { "; 3. Transient SRP (no persistent identity verification)" })?;
        let method = match numbered_choice(input, output, if enrollment { 2 } else { 3 }, "method")?
        {
            0 => Method::PinDisplay,
            1 => Method::SetupCode,
            2 => Method::Transient,
            _ => unreachable!(),
        };
        super::validate_selection(loaded, &id, method, snapshot)?
    };
    if enrollment {
        enrollment_prerequisite(selected.method, input, output)?;
    }
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
    fn status_priority_and_combinations() {
        for (approved, selected, paired, label) in [
            (false, false, false, "Not approved"),
            (false, true, false, "Not approved"),
            (false, false, true, "Not approved"),
            (false, true, true, "Not approved"),
            (true, false, false, "Not paired"),
            (true, true, false, "Selected"),
            (true, false, true, "Paired"),
            (true, true, true, "Paired, selected"),
        ] {
            assert_eq!(
                ReceiverState {
                    approved,
                    selected,
                    paired
                }
                .label(),
                label
            );
        }
    }

    #[test]
    fn compact_long_names_and_empty_results() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded = Loaded {
            config: crate::config::Config::default(),
            path: tmp.path().join("config.toml"),
            source: "test",
        };
        let receivers = vec![Receiver {
            name: "Long receiver ".repeat(20),
            id: "device:aa1122334455".into(),
            services: vec![],
        }];
        let mut output = Vec::new();
        listing(&loaded, &receivers, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text.lines().count(), 3);
        assert!(text.contains("…  [Not approved]"));
        assert!(!text.contains("Device ID:"));
        assert!(!short_name("Room\n\u{1b}[31m").chars().any(char::is_control));
        let mut output = Vec::new();
        assert!(choose(&loaded, &[], &mut "1\n".as_bytes(), &mut output).is_err());
        assert!(String::from_utf8(output)
            .unwrap()
            .contains("No receivers discovered"));
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
        assert!(output.contains("[Not approved]"));
        assert_eq!(output.lines().count(), 4);
        assert!(output.contains("(device:aa1122334455)"));
        assert!(!output.contains("Ready"));
        assert!(require_approval(&loaded, &receivers[0].id).is_err());
        assert!(choose(&loaded, &receivers, &mut "1\n".as_bytes(), &mut Vec::new()).is_err());
        let mut loaded = loaded;
        loaded.config.receiver_allowlist = receivers.iter().map(|r| r.id.clone()).collect();
        for (input, expected) in [
            ("1\n", "device:aa1122334455"),
            ("2\n", "device:bb1122334455"),
        ] {
            assert_eq!(
                choose(&loaded, &receivers, &mut input.as_bytes(), &mut Vec::new()).unwrap(),
                expected
            );
        }
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
    fn stored_hap_picker_uses_existing_credentials_and_confirmation() {
        let f = Fixture::new(true);
        let before = f.files();
        let proposed = f.propose("1\n1\n", false).unwrap().unwrap();
        assert_eq!(proposed.selected.id, NEW);
        assert_eq!(proposed.selected.method, Method::PinDisplay);
        assert_eq!(f.files(), before);
        assert!(f.authorize(&proposed, "n\n").is_err());
        assert_eq!(f.files(), before);
        f.authorize(&proposed, "yes\n").unwrap();
        assert!(
            Store::new(&f.loaded.path)
                .unwrap()
                .selection()
                .unwrap()
                .unwrap()
                == proposed.selected
        );
        let f = Fixture::new(false);
        let before = f.files();
        let proposed = f.propose("1\n1\n", false).unwrap().unwrap();
        assert!(f.authorize(&proposed, "yes\n").is_err());
        assert_eq!(f.files(), before);
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
                    .propose(&format!("1\n{method}\ny\n"), enrollment)
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
    fn enrollment_prerequisites_are_explicit_and_cancellation_preserves_every_file() {
        for (method, phrase) in [
            ("1", "receiver-supported PIN display"),
            ("2", "genuine, independently obtained HAP setup code"),
        ] {
            let f = Fixture::new(false);
            let before = f.files();
            for answer in ["", "y", "\n", "n\n", "q\n", "invented\n"] {
                assert!(f.propose(&format!("1\n{method}\n{answer}"), true).is_err());
                assert_eq!(f.files(), before);
            }
            let mut output = Vec::new();
            let proposed = prepare(
                &f.loaded,
                &f.snapshot,
                &mut format!("1\n{method}\nyes\n").as_bytes(),
                &mut output,
                true,
            )
            .unwrap()
            .unwrap();
            let text = String::from_utf8(output).unwrap();
            for expected in [
                phrase,
                "permanent verified credentials",
                "separate from session authentication and playback",
                "not an AirPlay password",
                "Do not invent codes or guess methods",
                "persistent enrollment cannot currently proceed",
                "No automatic fallback",
            ] {
                assert!(text.contains(expected), "{expected}: {text}");
            }
            assert_eq!(f.files(), before);
            assert!(f.authorize(&proposed, "").is_err());
            assert_eq!(f.files(), before);
        }
    }

    #[test]
    fn saved_method_still_requires_prerequisite_and_never_falls_back() {
        for method in [Method::PinDisplay, Method::SetupCode, Method::Transient] {
            let f = Fixture::new(false);
            super::super::select_from_snapshot(&f.loaded, NEW, method, &f.snapshot).unwrap();
            let before = f.files();
            assert!(f.propose("", true).is_err()); // no receiver selected automatically
            for input in ["1\n", "1\nn\n"] {
                assert!(f.propose(input, true).is_err());
                assert_eq!(f.files(), before);
            }
            if method != Method::Transient {
                let proposed = f.propose("1\ny\n", true).unwrap().unwrap();
                assert_eq!(proposed.selected.method, method);
                assert_eq!(f.files(), before);
            } else {
                assert!(f
                    .propose("1\ny\n", true)
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("does not enroll credentials"));
            }
        }
    }

    #[test]
    fn names_and_models_never_choose_a_method_and_transient_is_not_paired() {
        for name in ["Even-Olai HomePod", "Apple TV", "Generic receiver"] {
            let mut f = Fixture::new(false);
            f.snapshot[0].name = name.into();
            f.snapshot[0].services[0]
                .capabilities
                .insert("model".into(), name.into());
            let before = f.files();
            assert!(f.propose("1\n", true).is_err());
            for (choice, method) in [("1", Method::PinDisplay), ("2", Method::SetupCode)] {
                let proposed = f
                    .propose(&format!("1\n{choice}\ny\n"), true)
                    .unwrap()
                    .unwrap();
                assert_eq!(proposed.selected.method, method);
                assert_eq!(f.files(), before);
            }
            let proposed = f.propose("1\n3\n", false).unwrap().unwrap();
            assert_eq!(proposed.selected.method, Method::Transient);
            f.authorize(&proposed, "y\n").unwrap();
            let mut output = Vec::new();
            listing(&f.loaded, &f.snapshot, &mut output).unwrap();
            let text = String::from_utf8(output).unwrap();
            assert!(text.contains("[Selected]"));
            assert!(!text.contains("[Paired"));
            assert!(Store::new(&f.loaded.path)
                .unwrap()
                .credential(NEW)
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn unavailable_persistent_method_does_not_try_transient_or_change_selection() {
        let mut f = Fixture::new(false);
        f.snapshot[0].name = "Even-Olai HomePod".into();
        f.snapshot[0].services[0]
            .capabilities
            .insert("features".into(), "0x0,0x10000".into());
        let before = f.files();
        for input in ["1\n", "1\n1\ny\n", "1\n2\ny\n"] {
            assert!(f.propose(input, true).is_err());
            assert_eq!(f.files(), before);
        }
    }

    #[test]
    fn detailed_snapshot_includes_full_identity_endpoints_and_local_states() {
        let mut f = Fixture::new(true);
        f.snapshot[0].services[0]
            .addresses
            .insert("::1".parse().unwrap());
        let mut output = Vec::new();
        render_listing(&f.loaded, &f.snapshot, &mut output, true).unwrap();
        let output = String::from_utf8(output).unwrap();
        for expected in [
            "New receiver",
            NEW,
            "127.0.0.2",
            "::1",
            "_airplay._tcp.local.",
            "port 7000",
            "Approved: true",
            "Selected: false",
            "Paired locally: true",
            "[Paired]",
        ] {
            assert!(output.contains(expected), "{expected}: {output}");
        }
        let old = Store::new(&f.loaded.path)
            .unwrap()
            .selection()
            .unwrap()
            .unwrap();
        f.snapshot[0].id = old.id;
        let mut output = Vec::new();
        listing(&f.loaded, &f.snapshot, &mut output).unwrap();
        assert!(String::from_utf8(output)
            .unwrap()
            .contains("[Paired, selected]"));
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
            let proposed = f.propose("1\n2\ny\n", enrollment).unwrap().unwrap();
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
                .propose("1\n1\ny\n", fault != "missing-credential")
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
        let proposed = f.propose("1\n1\ny\n", true).unwrap().unwrap();
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
        let proposed = f.propose("1\n1\ny\n", true).unwrap().unwrap();
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
