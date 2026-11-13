use super::*;
use std::{collections::BTreeMap, io::BufRead};
fn scripted(input: &str) -> Ui<impl FnMut() -> Result<String> + '_, Vec<u8>> {
    let mut input = input.as_bytes();
    Ui {
        read: move || {
            let mut line = String::new();
            input.read_line(&mut line)?;
            Ok(line)
        },
        out: Vec::new(),
    }
}
const ID: &str = "device:112233445566";
fn fixture() -> (tempfile::TempDir, Loaded, Vec<Receiver>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "# keep me\n[audio]\ndevice='legacy'\n").unwrap();
    let loaded = config::load(Some(path)).unwrap();
    let info = mdns_sd::ServiceInfo::new(
        "_airplay._tcp.local.",
        "Room",
        "room.local.",
        "127.0.0.2",
        7000,
        [
            ("deviceid", "11:22:33:44:55:66"),
            ("features", "0x80200,0x10200"),
            ("sf", "0x0"),
        ]
        .as_slice(),
    )
    .unwrap();
    let rows = vec![Receiver {
        id: ID.into(),
        name: "Room".into(),
        services: vec![discovery::Advertisement::test_from_info(&info)],
    }];
    (dir, loaded, rows)
}
#[test]
fn approval_requires_both_explicit_confirmations_and_preserves_config() {
    for answer in ["2\n", "1\n2\n", "", "1\n"] {
        let (_d, mut loaded, _) = fixture();
        let before = std::fs::read(&loaded.path).unwrap();
        let mut journal = Vec::new();
        assert!(approve(&mut loaded, ID, &mut scripted(answer), &mut journal).is_err());
        assert_eq!(std::fs::read(&loaded.path).unwrap(), before);
        assert!(journal.is_empty());
    }
    let (_d, mut loaded, _) = fixture();
    let mut journal = Vec::new();
    approve(&mut loaded, ID, &mut scripted("1\n1\n"), &mut journal).unwrap();
    assert!(loaded.config.allows(ID));
    assert_eq!(journal.len(), 1);
    let before = std::fs::read(&loaded.path).unwrap();
    approve(&mut loaded, ID, &mut scripted(""), &mut journal).unwrap();
    assert_eq!(std::fs::read(&loaded.path).unwrap(), before);
    assert!(String::from_utf8(before).unwrap().contains("# keep me"));
}
#[test]
fn missing_receivers_rescan_cancel_and_ambiguous_identity() {
    let (_d, loaded, mut rows) = fixture();
    assert!(choose_receiver(&loaded, &[], &mut scripted("1\n"))
        .unwrap()
        .is_none());
    assert!(choose_receiver(&loaded, &[], &mut scripted("2\n")).is_err());
    assert_eq!(
        choose_receiver(&loaded, &rows, &mut scripted("1\n"))
            .unwrap()
            .as_deref(),
        Some(ID)
    );
    assert!(choose_receiver(&loaded, &rows, &mut scripted("2\n"))
        .unwrap()
        .is_none());
    rows.push(Receiver {
        id: ID.into(),
        name: "Duplicate".into(),
        services: vec![],
    });
    assert!(choose_receiver(&loaded, &rows, &mut scripted("1\n")).is_err());
}
#[test]
fn transient_requires_confirmation_and_never_enrolls_or_falls_back() {
    for input in ["", "1\n2\n", "1\n1\n2\n"] {
        let (_d, mut loaded, rows) = fixture();
        let mut journal = Vec::new();
        approve(&mut loaded, ID, &mut scripted("1\n1\n"), &mut journal).unwrap();
        assert!(authentication(
            &loaded,
            &rows,
            ID,
            &mut scripted(input),
            &mut journal,
            &Arc::new(AtomicBool::new(false))
        )
        .is_err());
        let store = Store::new(&loaded.path).unwrap();
        assert!(store.selection().unwrap().is_none());
        assert!(store.credential(ID).unwrap().is_none());
    }
    let (_d, mut loaded, rows) = fixture();
    let mut journal = Vec::new();
    approve(&mut loaded, ID, &mut scripted("1\n1\n"), &mut journal).unwrap();
    assert_eq!(
        authentication(
            &loaded,
            &rows,
            ID,
            &mut scripted("1\n1\n1\n"),
            &mut journal,
            &Arc::new(AtomicBool::new(false))
        )
        .unwrap(),
        Authentication::OwnToneTransient
    );
    let store = Store::new(&loaded.path).unwrap();
    assert!(store.credential(ID).unwrap().is_none());
    assert_eq!(
        store.selection().unwrap().unwrap().method,
        Method::Transient
    );
    // Cancelling the rest of setup retains explicit approval and selection.
    assert_eq!(journal.len(), 2);
    let baseline = allowlist::snapshot(&loaded.path).unwrap();
    let draft = Playback {
        capture_device: Some("null".into()),
        receiver_id: Some(ID.into()),
        authentication: Some(Authentication::OwnToneTransient),
        ..Default::default()
    };
    config::save_playback_checked(
        &loaded,
        Some(ID.into()),
        playback_args(&draft),
        Some(&baseline),
    )
    .unwrap();
    let saved = config::load(Some(loaded.path.clone())).unwrap();
    assert_eq!(saved.config.audio.device, "legacy");
    assert!(saved.config.allows(ID));
    assert!(std::fs::read_to_string(&loaded.path)
        .unwrap()
        .contains("# keep me"));
    // A stale review must not replace a newly edited file.
    assert!(config::save_playback_checked(
        &saved,
        Some(ID.into()),
        playback_args(&draft),
        Some(&baseline)
    )
    .is_err());
}
#[test]
fn measurement_suggestions_require_real_separation() {
    assert_eq!(suggested_threshold(&[-70.0; 25], &[-30.0; 25]), Some(-42.0));
    assert_eq!(suggested_threshold(&[-44.0; 25], &[-30.0; 25]), Some(-38.0));
    assert!(suggested_threshold(&[-40.0; 25], &[-35.0; 25]).is_none());
    assert!(suggested_threshold(&[], &[]).is_none());
    assert!(suggested_threshold(&[], &[-30.0; 25]).is_none());
    assert!(suggested_threshold(&[-70.0; 25], &[]).is_none());
    assert!(suggested_threshold(&[-70.0; 25], &[-70.0; 25]).is_none());
    assert!(suggested_threshold(&[f64::NEG_INFINITY; 25], &[f64::NEG_INFINITY; 25]).is_none());
    assert_eq!(
        suggested_threshold(&[f64::NEG_INFINITY; 25], &[-30.0; 25]),
        Some(-42.0)
    );
    // Digital zero must not justify following an unusably weak input.
    assert!(suggested_threshold(&[f64::NEG_INFINITY; 25], &[-90.0; 25]).is_none());
    assert_eq!(suggested_threshold(&[-90.0; 25], &[-54.0; 25]), Some(-60.0));
}
#[test]
fn suggestions_reject_invalid_levels_and_use_quieter_music() {
    for invalid in [f64::NAN, f64::INFINITY, -121.0, 0.1] {
        // Reject invalid samples even when they would be outside a percentile.
        let mut noise = [-70.0; 25];
        noise[0] = invalid;
        assert!(suggested_threshold(&noise, &[-30.0; 25]).is_none());
        let mut music = [-30.0; 25];
        music[24] = invalid;
        assert!(suggested_threshold(&[-70.0; 25], &music).is_none());
    }
    let mut music = [-20.0; 25];
    music[..7].fill(-40.0);
    assert_eq!(suggested_threshold(&[-70.0; 25], &music), Some(-52.0));
    music[..7].fill(f64::NEG_INFINITY);
    assert!(suggested_threshold(&[-70.0; 25], &music).is_none());
}

#[test]
fn failed_playback_reports_guidance_without_reading_success_confirmation() {
    let mut ui = Ui {
        read: || panic!("failed playback must not ask for audible-success confirmation"),
        out: Vec::new(),
    };
    playback_feedback(
        Err(anyhow::anyhow!(
            "mock authentication or transmission failure"
        )),
        &mut ui,
        &AtomicBool::new(false),
    )
    .unwrap();
    let text = String::from_utf8(ui.out).unwrap();
    assert!(text.contains("Playback test failed: mock authentication or transmission failure"));
    assert!(text.contains("Check receiver volume/mute"));
    assert!(text.contains("Continuing setup"));
    assert!(!text.contains("Did you hear the audio?"));
    assert!(!text.contains("Yes, audio works"));
}

#[test]
fn successful_playback_still_requires_user_confirmation_of_audibility() {
    for (input, expected) in [
        ("1\n", "You confirmed audible output"),
        ("2\n", "Audio is not verified"),
        ("3\n", "Audible playback remains unverified"),
    ] {
        let mut ui = scripted(input);
        playback_feedback(Ok(()), &mut ui, &AtomicBool::new(false)).unwrap();
        let text = String::from_utf8(ui.out).unwrap();
        assert!(text.contains("Did you hear the audio?"));
        assert!(text.contains(expected));
    }
}

#[test]
fn playback_cancellation_and_teardown_failure_remain_errors_without_confirmation() {
    let mut ui = Ui {
        read: || panic!("cancelled playback must not prompt"),
        out: Vec::new(),
    };
    let cancel = AtomicBool::new(true);
    assert!(playback_feedback(Ok(()), &mut ui, &cancel)
        .unwrap_err()
        .is::<Cancelled>());
    let cleanup = super::super::lifecycle::CleanupFailure(anyhow::anyhow!("mock TEARDOWN failed"));
    assert!(playback_feedback(Err(cleanup.into()), &mut ui, &cancel)
        .unwrap_err()
        .is::<super::super::lifecycle::CleanupFailure>());
    assert!(ui.out.is_empty());
}
#[test]
fn invalid_manual_thresholds_retry_without_changing_draft() {
    let mut draft = Playback::default();
    configure_activity(
        &mut draft,
        &mut scripted("2\nnan\n1000\n30000\n1\n"),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(draft.threshold_dbfs, -45.0);
    configure_activity(
        &mut draft,
        &mut scripted("2\n-50\n2000\n40000\n1\n"),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(draft.threshold_dbfs, -50.0);
    // The conservative suggestion floor must not restrict deliberate manual input.
    configure_activity(
        &mut draft,
        &mut scripted("2\n-100\n1000\n30000\n1\n"),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(draft.threshold_dbfs, -100.0);
}
#[test]
fn default_skip_does_not_touch_receiver_or_service() {
    let (_d, loaded, _) = fixture();
    let mut ui = scripted("\n");
    test_audio(
        &loaded,
        &Playback::default(),
        &mut ui,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert!(Store::new(&loaded.path)
        .unwrap()
        .selection()
        .unwrap()
        .is_none());
    // Explicit No/EOF are already covered by Ui; this host-independent assertion
    // verifies no command invocation is reachable through default confirmation.
    assert!(!scripted("\n").confirm("Enable service?").unwrap());
}
#[test]
fn names_are_printable() {
    assert_eq!(clean("Room\u{1b}\n"), "Room  ");
}
#[test]
fn unsupported_advertisement_never_selects() {
    let (_d, mut loaded, mut rows) = fixture();
    let mut journal = Vec::new();
    approve(&mut loaded, ID, &mut scripted("1\n1\n"), &mut journal).unwrap();
    rows[0].services[0].capabilities = BTreeMap::from([("features".into(), "0x0".into())]);
    assert!(authentication(
        &loaded,
        &rows,
        ID,
        &mut scripted("1\n1\n1\n"),
        &mut journal,
        &Arc::new(AtomicBool::new(false))
    )
    .is_err());
    assert!(Store::new(&loaded.path)
        .unwrap()
        .selection()
        .unwrap()
        .is_none());
}

#[test]
fn stored_credentials_are_reused_without_reenrollment_or_modification() {
    let (_d, mut loaded, mut rows) = fixture();
    let mut journal = Vec::new();
    approve(&mut loaded, ID, &mut scripted("1\n1\n"), &mut journal).unwrap();
    rows[0].services[0]
        .capabilities
        .insert("features".into(), "0x80200,0x14200".into());
    let store = Store::new(&loaded.path).unwrap();
    let selected = super::super::validate_selection(&loaded, ID, Method::SetupCode, &rows).unwrap();
    store.select(&selected).unwrap();
    store
        .save(&super::super::store::Credential {
            version: 2,
            receiver_id: ID.into(),
            pairing_id: "verified".into(),
            advertised_pi: None,
            controller_id: "00000000-0000-4000-8000-000000000001".into(),
            seed: [11; 32],
            receiver_key: ed25519_dalek::SigningKey::from_bytes(&[22; 32])
                .verifying_key()
                .to_bytes(),
        })
        .unwrap();
    let path = loaded
        .path
        .parent()
        .unwrap()
        .join("receiver-state/112233445566.toml");
    let before = std::fs::read(&path).unwrap();
    let mut ui = scripted("1\n1\n");
    assert_eq!(
        authentication(
            &loaded,
            &rows,
            ID,
            &mut ui,
            &mut journal,
            &Arc::new(AtomicBool::new(false))
        )
        .unwrap(),
        Authentication::StoredHap
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(store.selection().unwrap().as_ref() == Some(&selected));
    assert!(!String::from_utf8(ui.out)
        .unwrap()
        .contains("PIN-display enrollment"));
}
#[test]
fn enrollment_requires_real_pin_or_setup_code_before_selection() {
    for method in [1, 2] {
        let (_d, mut loaded, mut rows) = fixture();
        let mut journal = Vec::new();
        approve(&mut loaded, ID, &mut scripted("1\n1\n"), &mut journal).unwrap();
        rows[0].services[0]
            .capabilities
            .insert("features".into(), "0x80200,0x14200".into());
        for answer in ["2\n", ""] {
            assert!(authentication(
                &loaded,
                &rows,
                ID,
                &mut scripted(&format!("{method}\n{answer}")),
                &mut journal,
                &Arc::new(AtomicBool::new(false))
            )
            .is_err());
            assert!(Store::new(&loaded.path)
                .unwrap()
                .selection()
                .unwrap()
                .is_none());
            assert!(Store::new(&loaded.path)
                .unwrap()
                .credential(ID)
                .unwrap()
                .is_none());
        }
    }
}

#[test]
fn mock_alsa_missing_devices_rescan_validation_failure_and_explicit_choice() {
    let mut scans = 0;
    let mut validated = Vec::new();
    let selected = select_input_with(
        &Playback::default(),
        &mut scripted("2\n1\nbusy\n1\nchosen\n"),
        &AtomicBool::new(false),
        || {
            scans += 1;
            Ok(vec![])
        },
        |id| {
            validated.push(id.to_owned());
            ensure!(id != "busy", "mock device busy");
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(selected, "chosen");
    assert_eq!(scans, 3);
    assert_eq!(validated, ["busy", "chosen"]);
    let mut calls = 0;
    assert!(select_input_with(
        &Playback::default(),
        &mut scripted(""),
        &AtomicBool::new(false),
        || Ok(vec![("only".into(), "Only device".into())]),
        |_| {
            calls += 1;
            Ok(())
        }
    )
    .is_err());
    assert_eq!(calls, 0); // Even one device is never auto-selected.
    for failure in [
        "unavailable",
        "permission denied",
        "busy",
        "unsupported format",
    ] {
        let mut ui = scripted("1\nq\n");
        assert!(select_input_with(
            &Playback::default(),
            &mut ui,
            &AtomicBool::new(false),
            || Ok(vec![("exact".into(), "Input".into())]),
            |id| {
                assert_eq!(id, "exact");
                anyhow::bail!("{failure}")
            }
        )
        .is_err());
        assert!(String::from_utf8(ui.out).unwrap().contains(failure));
    }
}
#[test]
fn discovery_failure_can_retry_and_is_cancellable() {
    let mut calls = 0;
    let rows = discover_receivers_with(5, &mut scripted("1\n"), &AtomicBool::new(false), |_, _| {
        calls += 1;
        if calls == 1 {
            anyhow::bail!("mock mDNS failure")
        }
        Ok(vec![])
    })
    .unwrap();
    assert!(rows.is_empty());
    assert_eq!(calls, 2);
    assert!(discover_receivers_with(
        5,
        &mut scripted("2\n"),
        &AtomicBool::new(false),
        |_, _| anyhow::bail!("failure")
    )
    .is_err());
}
#[test]
fn service_activation_needs_installed_unit_and_both_affirmative_choices() {
    let (_d, loaded, _) = fixture();
    for input in ["\n", "2\n", "1\n2\n", "1\n\n", "", "1\n"] {
        let mut invoked = false;
        let _ = optional_service_with(
            &loaded,
            &mut scripted(input),
            &mut Vec::new(),
            |_| {
                Ok(service::Service {
                    unit: "piair@pi.service".into(),
                    status: "inactive, disabled".into(),
                    home: "/home/pi".into(),
                    user: "pi".into(),
                })
            },
            |_, _| {
                invoked = true;
                Ok(())
            },
        );
        assert!(!invoked, "{input:?}");
    }
    let mut invoked = false;
    let mut journal = Vec::new();
    optional_service_with(
        &loaded,
        &mut scripted("1\n1\n"),
        &mut journal,
        |_| {
            Ok(service::Service {
                unit: "piair@pi.service".into(),
                status: "inactive, disabled".into(),
                home: "/home/pi".into(),
                user: "pi".into(),
            })
        },
        |_, s| {
            assert_eq!(s.unit, "piair@pi.service");
            invoked = true;
            Ok(())
        },
    )
    .unwrap();
    assert!(invoked);
    assert_eq!(journal.len(), 1);
    optional_service_with(
        &loaded,
        &mut scripted("1\n1\n"),
        &mut Vec::new(),
        |_| anyhow::bail!("unit missing"),
        |_, _| panic!("must not activate missing unit"),
    )
    .unwrap();
}
