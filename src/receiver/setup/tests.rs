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
fn approval_requires_one_explicit_confirmation_and_preserves_config() {
    for answer in ["2\n", "", "q\n"] {
        let (_d, mut loaded, _) = fixture();
        let before = std::fs::read(&loaded.path).unwrap();
        let mut journal = Vec::new();
        assert!(approve(&mut loaded, ID, &mut scripted(answer), &mut journal).is_err());
        assert_eq!(std::fs::read(&loaded.path).unwrap(), before);
        assert!(journal.is_empty());
    }
    let (_d, mut loaded, _) = fixture();
    let mut journal = Vec::new();
    approve(&mut loaded, ID, &mut scripted("1\n"), &mut journal).unwrap();
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
        approve(&mut loaded, ID, &mut scripted("1\n"), &mut journal).unwrap();
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
    approve(&mut loaded, ID, &mut scripted("1\n"), &mut journal).unwrap();
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
    assert!(text.contains("couldn't prepare the audio test"));
    assert!(!text.contains("mock authentication or transmission failure"));
    assert!(text.contains("piair -v setup"));
    assert!(text.contains("No successful playback test recorded"));
    assert!(!text.contains("Did you hear the audio?"));
    assert!(!text.contains("Yes, audio works"));
}

#[test]
fn successful_playback_still_requires_user_confirmation_of_audibility() {
    for (input, expected) in [
        ("1\n", "You confirmed audible output"),
        ("2\n", "couldn't confirm that sound was heard"),
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
    recording::offer(
        &loaded,
        "unused when skipped",
        &mut ui,
        &AtomicBool::new(false),
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
    approve(&mut loaded, ID, &mut scripted("1\n"), &mut journal).unwrap();
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
    approve(&mut loaded, ID, &mut scripted("1\n"), &mut journal).unwrap();
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
        approve(&mut loaded, ID, &mut scripted("1\n"), &mut journal).unwrap();
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
        &mut scripted("2\n1\n1\nbusy\n1\n1\nchosen\n"),
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
                    active: "inactive".into(),
                    boot: "disabled".into(),
                    pid: 0,
                    result: "success".into(),
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
                active: "inactive".into(),
                boot: "disabled".into(),
                pid: 0,
                result: "success".into(),
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

#[test]
fn normal_input_list_prefers_conversion_and_preserves_manual_selection() {
    let devices = vec![
        ("hw:CARD=USB,DEV=0".into(), "USB".into()),
        ("plughw:CARD=USB,DEV=0".into(), "USB".into()),
        ("hw:CARD=Other,DEV=0".into(), "Other".into()),
    ];
    let mut ui = scripted("1\n");
    assert_eq!(
        select_input_with(
            &Playback::default(),
            &mut ui,
            &AtomicBool::new(false),
            || Ok(devices.clone()),
            |_| Ok(())
        )
        .unwrap(),
        "plughw:CARD=USB,DEV=0"
    );
    assert_eq!(normal_inputs(devices).len(), 2);
    assert_eq!(
        select_input_with(
            &Playback::default(),
            &mut scripted("1\n1\nhw:manual\n"),
            &AtomicBool::new(false),
            || Ok(vec![]),
            |_| Ok(())
        )
        .unwrap(),
        "hw:manual"
    );
}

#[test]
fn normal_receiver_screen_has_only_names_and_explicit_selection() {
    let (_d, loaded, rows) = fixture();
    let mut ui = scripted("1\n");
    assert_eq!(
        choose_receiver(&loaded, &rows, &mut ui).unwrap().as_deref(),
        Some(ID)
    );
    let text = String::from_utf8(ui.out).unwrap();
    assert!(text.contains("Room"));
    for technical in [
        ID,
        "127.0.0.2",
        "features",
        "Endpoint",
        "Approved:",
        "Capabilities",
    ] {
        assert!(!text.contains(technical));
    }
}

#[test]
fn normal_defaults_and_advanced_adjustment() {
    let mut draft = Playback::default();
    let before = draft.clone();
    let mut ui = scripted("\n");
    advanced_settings(&mut draft, &mut ui, &AtomicBool::new(false)).unwrap();
    assert_eq!(draft.threshold_dbfs, before.threshold_dbfs);
    assert_eq!(draft.volume_db, before.volume_db);
    let text = String::from_utf8(ui.out).unwrap();
    assert!(!text.contains("dBFS"));
    advanced_settings(
        &mut draft,
        &mut scripted("2\n2\n-50\n2000\n40000\n1\n2\n"),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(draft.threshold_dbfs, -50.0);
    assert_eq!(draft.volume_db, None);
}

#[test]
fn playback_discovery_retries_absence_only_and_keeps_binding() {
    let (_d, mut loaded, rows) = fixture();
    approve(&mut loaded, ID, &mut scripted("1\n"), &mut Vec::new()).unwrap();
    let selected = super::super::validate_selection(&loaded, ID, Method::Transient, &rows).unwrap();
    let mut scans = 0;
    let mut final_rows = Some(rows);
    let result = super::super::playback_discovery(
        &loaded,
        &selected,
        &AtomicBool::new(false),
        |timeout, _| {
            assert_eq!(timeout, 5);
            scans += 1;
            Ok(if scans == 1 {
                vec![]
            } else {
                final_rows.take().unwrap()
            })
        },
    )
    .unwrap();
    assert_eq!(scans, 2);
    assert_eq!(result[0].id, ID);
    scans = 0;
    let error =
        super::super::playback_discovery(&loaded, &selected, &AtomicBool::new(false), |_, _| {
            scans += 1;
            Ok(vec![])
        })
        .unwrap_err();
    assert_eq!(scans, 3);
    assert!(format!("{error:#}").contains("authentication not started"));
    super::super::playback_discovery(&loaded, &selected, &AtomicBool::new(true), |_, _| {
        panic!("cancelled discovery must not start")
    })
    .unwrap_err();
    scans = 0;
    let cancel = AtomicBool::new(false);
    super::super::playback_discovery(&loaded, &selected, &cancel, |_, cancel| {
        scans += 1;
        cancel.store(true, Ordering::Relaxed);
        Ok(vec![])
    })
    .unwrap_err();
    assert_eq!(scans, 1);
    for change in ["capability", "conflict", "duplicate", "endpoint"] {
        let (_, _, mut rows) = fixture();
        match change {
            "capability" => {
                rows[0].services[0]
                    .capabilities
                    .insert("sf".into(), "0x80".into());
            }
            "conflict" => rows[0].id = "device:aabbccddeeff".into(),
            "duplicate" => rows.push(Receiver {
                id: ID.into(),
                name: "Duplicate".into(),
                services: vec![],
            }),
            _ => {
                rows[0].services[0].port += 1;
            }
        }
        let mut rows = Some(rows);
        scans = 0;
        assert!(
            super::super::playback_discovery(
                &loaded,
                &selected,
                &AtomicBool::new(false),
                |_, _| {
                    scans += 1;
                    Ok(rows.take().unwrap())
                }
            )
            .is_err(),
            "{change}"
        );
        assert_eq!(scans, 1);
    }
}

#[test]
fn friendly_approval_is_explicit_and_invalid_canonical_id_never_prompts() {
    let (_d, mut loaded, _) = fixture();
    let mut ui = scripted("1\n");
    approve_named(&mut loaded, ID, "Living room", &mut ui, &mut Vec::new()).unwrap();
    let text = String::from_utf8(ui.out).unwrap();
    assert!(text.contains("Approve speaker \"Living room\"?"));
    assert!(!text.contains(ID));
    let before = std::fs::read(&loaded.path).unwrap();
    let mut ui = Ui {
        read: || panic!("invalid identity must not prompt"),
        out: Vec::new(),
    };
    assert!(approve_named(
        &mut loaded,
        "device:invalid",
        "Room",
        &mut ui,
        &mut Vec::new()
    )
    .is_err());
    assert_eq!(std::fs::read(&loaded.path).unwrap(), before);
}

#[test]
fn normal_wizard_steps_use_plain_labels_and_default_settings() {
    let (_d, mut loaded, rows) = fixture();
    let mut draft = Playback::default();
    let cancel = Arc::new(AtomicBool::new(false));
    // One affirmative input choice, speaker choice, approval, method choice,
    // session-only warning and selection save, followed by default settings/test.
    let mut ui = scripted("1\n1\n1\n1\n1\n1\n\n\n");
    draft.capture_device = Some(
        select_input_with(
            &draft,
            &mut ui,
            &cancel,
            || Ok(vec![("plughw:USB".into(), "USB input".into())]),
            |_| Ok(()),
        )
        .unwrap(),
    );
    let id = choose_receiver(&loaded, &rows, &mut ui).unwrap().unwrap();
    let mut journal = Vec::new();
    approve_named(&mut loaded, &id, &rows[0].name, &mut ui, &mut journal).unwrap();
    draft.authentication =
        Some(authentication(&loaded, &rows, &id, &mut ui, &mut journal, &cancel).unwrap());
    advanced_settings(&mut draft, &mut ui, &cancel).unwrap();
    assert!(!test_audio_requested(&mut ui).unwrap());
    assert_eq!(draft.volume_db, None);
    assert_eq!(draft.threshold_dbfs, -45.0);
    assert_eq!(journal.len(), 2);
    let text = String::from_utf8(ui.out).unwrap();
    assert!(text.contains("Connect without saving pairing"));
    assert!(text.contains("does not save a pairing or verify a persistent speaker identity"));
    for technical in [
        "OwnTone",
        "SRP",
        "ALAC",
        "PTP",
        "RTP",
        "Installer planned",
        "127.0.0.2",
        "dBFS",
    ] {
        assert!(!text.contains(technical), "{technical}");
    }
}

#[test]
fn playback_failure_stages_keep_details_only_in_verbose_output() {
    use super::super::playback_error::Stage;
    for (stage, message) in [
        (Stage::Discovery, "couldn't find or validate"),
        (Stage::Connection, "found the speaker but couldn't connect"),
        (
            Stage::Playback,
            "connected to the speaker but couldn't play",
        ),
    ] {
        for detailed in [false, true] {
            let error = anyhow::anyhow!("SRP HAP PTP ALAC RTP UDP 319/320 original diagnostic")
                .context("full nested cause")
                .context(stage);
            let mut ui = Ui {
                read: || panic!("failure must not ask if audio was heard"),
                out: Vec::new(),
            };
            playback_feedback_details(Err(error), &mut ui, &AtomicBool::new(false), detailed)
                .unwrap();
            let text = String::from_utf8(ui.out).unwrap();
            assert!(text.contains(message));
            assert!(!text.contains("You confirmed audible output"));
            for detail in [
                "SRP",
                "HAP",
                "PTP",
                "ALAC",
                "RTP",
                "319/320",
                "full nested cause",
                "original diagnostic",
            ] {
                assert_eq!(text.contains(detail), detailed, "{text}");
            }
        }
    }
    for cancel in [false, true] {
        let error = super::super::lifecycle::CleanupFailure(anyhow::anyhow!(
            "original serious cleanup error"
        ));
        let mut ui = scripted("");
        let result =
            playback_feedback_details(Err(error.into()), &mut ui, &AtomicBool::new(cancel), false)
                .unwrap_err();
        assert!(result.is::<super::super::lifecycle::CleanupFailure>());
        assert!(format!("{result:#}").contains("original serious cleanup error"));
        assert!(command_error_message(&result, false).contains("Setup stopped"));
        assert!(!command_error_message(&result, false).contains("original serious cleanup error"));
        assert!(command_error_message(&result, true).contains("original serious cleanup error"));
    }
}

#[test]
fn aliases_group_by_card_and_device_while_virtual_and_bluetooth_routes_survive() {
    let ids = [
        "default:CARD=CODEC",
        "dsnoop:CARD=CODEC,DEV=0",
        "front:CARD=CODEC,DEV=0",
        "hw:CARD=CODEC,DEV=0",
        "plughw:CARD=CODEC,DEV=0",
        "sysdefault:CARD=CODEC",
        "hw:CARD=Second,DEV=0",
        "plughw:CARD=Second,DEV=0",
        "plughw:CARD=Second,DEV=1",
        "hw:CARD=Loopback,DEV=0",
        "hw:CARD=Loopback,DEV=1",
        "dsnoop:CARD=Loopback,DEV=1",
        "bluealsa:DEV=11:22:33:44:55:66,PROFILE=sco",
        "virtual-loopback",
        "nonstandard:CARD=CODEC",
        "hw:CARD=CODEC,DEV=0,SUBDEV=1",
    ];
    // Identical descriptions on two cards must not merge their inputs.
    let devices: Vec<_> = ids
        .iter()
        .map(|id| {
            (
                id.to_string(),
                "USB Audio CODEC, technical PCM description".into(),
            )
        })
        .collect();
    let primary = normal_inputs(devices.clone());
    assert_eq!(primary.len(), 9);
    assert!(primary.contains(&("plughw:CARD=CODEC,DEV=0".into(), "USB Audio CODEC".into())));
    for id in [
        &ids[7], &ids[8], &ids[9], &ids[10], &ids[12], &ids[13], &ids[14], &ids[15],
    ] {
        assert!(primary.iter().any(|(selected, _)| selected == *id), "{id}");
    }
    // Every original alias can still be explicitly selected, without rewriting.
    for (index, id) in ids.iter().enumerate() {
        let answers = format!("{}\n", index + 1);
        let mut choice = scripted(&answers);
        assert_eq!(
            advanced_input(&devices, &Playback::default(), &mut choice)
                .unwrap()
                .as_deref(),
            Some(*id)
        );
    }
    let mut validated = Vec::new();
    let mut ui = scripted("1\nq\n");
    assert!(select_input_with(
        &Playback::default(),
        &mut ui,
        &AtomicBool::new(false),
        || Ok(devices.clone()),
        |id| {
            validated.push(id.to_owned());
            anyhow::bail!("device busy")
        }
    )
    .is_err());
    assert_eq!(validated, ["plughw:CARD=CODEC,DEV=0"]); // No silent hw/default retry.
}

#[test]
fn playback_feedback_honors_normal_verbose_and_trace_logging_levels() {
    for (level, detailed) in [
        (tracing::Level::INFO, false),
        (tracing::Level::DEBUG, true),
        (tracing::Level::TRACE, true),
    ] {
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(level)
            .with_writer(std::io::sink)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let mut ui = scripted("");
            let error = anyhow::anyhow!("original SRP diagnostic")
                .context(super::super::playback_error::Stage::Connection);
            playback_feedback(Err(error), &mut ui, &AtomicBool::new(false)).unwrap();
            let text = String::from_utf8(ui.out).unwrap();
            assert_eq!(text.contains("original SRP diagnostic"), detailed);
            assert!(text.contains("found the speaker but couldn't connect"));
        });
    }
}

#[test]
fn local_ptp_permission_failure_never_blames_speaker_or_requests_audible_confirmation() {
    for detailed in [false, true] {
        for port in [319, 320] {
            let error = super::super::wire::ptp_bind_error(
                port,
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            )
            .context(super::super::playback_error::Stage::Playback);
            let mut ui = Ui {
                read: || panic!("permission failure cannot establish audible success"),
                out: Vec::new(),
            };
            playback_feedback_details(Err(error), &mut ui, &AtomicBool::new(false), detailed)
                .unwrap();
            let text = String::from_utf8(ui.out).unwrap();
            assert!(text.contains("foreground process"));
            assert!(text.contains("piair start"));
            assert!(text.contains("Do not run PiAir as root"));
            assert!(text.contains("No successful playback test recorded"));
            if !detailed {
                assert!(!text.contains("Check the speaker and network"));
            }
            assert_eq!(text.contains(&format!("local PTP UDP {port}")), detailed);
        }
    }
}

#[test]
fn setup_operation_guidance_hides_protocol_details_but_keeps_cleanup_failures() {
    let error = anyhow::anyhow!("internal protocol rejection");
    for detailed in [false, true] {
        let text = setup_problem(&error, "Check the selected connection method.", detailed);
        assert!(text.contains("Check the selected connection method."));
        assert_eq!(text.contains("internal protocol rejection"), detailed);
    }
    let cleanup: anyhow::Error =
        super::super::lifecycle::CleanupFailure(anyhow::anyhow!("cleanup failed")).into();
    assert!(setup_problem(&cleanup, "Setup failed.", false).contains("cleanup failed"));
}

#[test]
fn isolated_candidate_never_offers_optional_service_activation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "").unwrap();
    let loaded = config::load(Some(path)).unwrap();
    let mut ui = Ui {
        read: || panic!("candidate must not offer enable/start confirmation"),
        out: Vec::new(),
    };
    optional_service_with(
        &loaded,
        &mut ui,
        &mut Vec::new(),
        service::inspect,
        |_, _| panic!("candidate must not activate installed service"),
    )
    .unwrap();
    let text = String::from_utf8(ui.out).unwrap();
    assert!(text.contains("Automatic activation unavailable"));
    assert!(text.contains("Service control is unavailable"));
    assert!(!text.contains("Start PiAir automatically when"));
    // Activation itself also guards against calling it without the initial probe.
    let expected = service::Service {
        unit: "piair@alice.service".into(),
        status: "inactive".into(),
        home: dir.path().into(),
        user: "alice".into(),
        active: "inactive".into(),
        boot: "disabled".into(),
        pid: 0,
        result: "success".into(),
    };
    assert!(service::activate(&loaded, &expected).is_err());
}
