use super::*;
use mdns_sd::ServiceInfo;

const G: &str = "11111111-1111-4111-8111-111111111111";
const H: &str = "22222222-2222-4222-8222-222222222222";
fn receiver(id: &str, service: &str, fields: &[(&str, &str)]) -> Receiver {
    let mut txt: std::collections::HashMap<String, String> = fields
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    txt.insert("deviceid".into(), id.into());
    let instance = if service.starts_with("_raop") {
        format!("{id}@Fixture")
    } else {
        "Fixture".into()
    };
    let info =
        ServiceInfo::new(service, &instance, "fixture.local.", "127.0.0.1", 7000, txt).unwrap();
    Receiver {
        id: format!("device:{id}"),
        name: "Fixture".into(),
        services: vec![Advertisement::test_from_info(&info)],
    }
}
fn ap(id: &str, extra: &[(&str, &str)]) -> Receiver {
    let mut fields = vec![("features", "0x80200,0x4000"), ("flags", "0x0")];
    fields.extend_from_slice(extra);
    receiver(id, "_airplay._tcp.local.", &fields)
}
fn authorized() -> Authorization {
    Authorization {
        allowlisted: true,
        method: Some(Method::SetupCode),
        requires_ptp: false,
    }
}
#[test]
fn synthetic_receiver_matrix() {
    // Product labels describe fixture scenarios only; implementation never reads them.
    for (label, r, eligible, buffered) in [
        ("Apple TV standalone", ap("aabbccddeeff", &[]), true, false),
        ("HomePod standalone", ap("aabbccddeeff", &[]), true, false),
        (
            "AirPlay 2 realtime only",
            ap("aabbccddeeff", &[]),
            true,
            false,
        ),
        (
            "buffered capable",
            receiver(
                "aabbccddeeff",
                "_airplay._tcp.local.",
                &[("features", "0x80200,0x4300"), ("flags", "0")],
            ),
            true,
            true,
        ),
        (
            "AirPlay 1 only",
            receiver(
                "aabbccddeeff",
                "_raop._tcp.local.",
                &[("cn", "0,1"), ("sr", "44100"), ("ss", "16"), ("ch", "2")],
            ),
            false,
            false,
        ),
        (
            "third party incomplete",
            receiver("aabbccddeeff", "_airplay._tcp.local.", &[]),
            false,
            false,
        ),
        (
            "malformed capabilities",
            receiver(
                "aabbccddeeff",
                "_airplay._tcp.local.",
                &[("features", "bad-mask")],
            ),
            false,
            false,
        ),
        (
            "password",
            ap("aabbccddeeff", &[("pw", "true")]),
            false,
            false,
        ),
        (
            "restricted home",
            ap("aabbccddeeff", &[("acl", "2")]),
            false,
            false,
        ),
        (
            "malformed format",
            ap("aabbccddeeff", &[("sr", "nonsense")]),
            false,
            false,
        ),
    ] {
        let caps = ReceiverCapabilities::observe(&r);
        assert_eq!(caps.buffered == Claim::Observed(true), buffered, "{label}");
        assert_eq!(caps.runtime, RuntimeResults::default());
        let t = topology(std::slice::from_ref(&r));
        let plan = plan(&r, authorized(), &t, None);
        assert_eq!(
            plan.iter().any(|c| c.rejections.is_empty()),
            eligible,
            "{label}: {plan:?}"
        );
        assert!(plan
            .iter()
            .filter(|c| c.transport != Transport::RealtimeNtp)
            .all(|c| !c.rejections.is_empty()));
        assert_eq!(plan, super::plan(&r, authorized(), &t, None));
    }
}
#[test]
fn authorization_and_timing_never_fall_back() {
    let r = ap("aabbccddeeff", &[]);
    let t = topology(std::slice::from_ref(&r));
    for auth in [
        Authorization::default(),
        Authorization {
            requires_ptp: true,
            ..authorized()
        },
        Authorization {
            method: None,
            ..authorized()
        },
        Authorization {
            method: Some(Method::Transient),
            ..authorized()
        },
    ] {
        assert!(plan(&r, auth, &t, None)
            .iter()
            .all(|c| !c.rejections.is_empty()));
    }
}
#[test]
fn proxy_stereo_conflicts_and_changing_snapshots_are_candidates_only() {
    for (label, snapshot, key) in [
        (
            "Apple TV and HomePod follower",
            vec![
                ap("aabbccddeeff", &[("gid", G), ("igl", "1")]),
                ap("112233445566", &[("gid", G), ("igl", "0")]),
            ],
            format!("gid:{G}"),
        ),
        (
            "Apple TV and two followers",
            vec![
                ap("aabbccddeeff", &[("gid", G), ("igl", "1")]),
                ap("112233445566", &[("gid", G), ("igl", "0")]),
                ap("223344556677", &[("gid", G), ("igl", "0")]),
            ],
            format!("gid:{G}"),
        ),
        (
            "HomePod stereo pair",
            vec![
                ap("aabbccddeeff", &[("gid", G), ("tsid", H), ("igl", "1")]),
                ap("112233445566", &[("gid", G), ("tsid", H), ("igl", "0")]),
            ],
            format!("tsid:{H}"),
        ),
    ] {
        let t = topology(&snapshot);
        assert_eq!(t.candidate_sets[&key].len(), snapshot.len(), "{label}");
        for r in &snapshot {
            assert!(plan(r, authorized(), &t, None)
                .iter()
                .all(|c| !c.rejections.is_empty()));
        }
        let newer = topology(&snapshot[..1]);
        assert_eq!(newer.candidate_sets[&key].len(), 1);
        assert!(topology(&[]).candidate_sets.is_empty());
    }
    let mut r = ap("aabbccddeeff", &[("gid", G)]);
    r.services
        .extend(ap("aabbccddeeff", &[("gid", H)]).services);
    assert!(topology(&[r]).ambiguous.contains("device:aabbccddeeff"));
    let shared_names = vec![ap("aabbccddeeff", &[]), ap("112233445566", &[])];
    assert!(topology(&shared_names).candidate_sets.is_empty());
    let competing = vec![
        ap("aabbccddeeff", &[("gid", G), ("igl", "1")]),
        ap("112233445566", &[("gid", G), ("igl", "1")]),
    ];
    assert_eq!(topology(&competing).ambiguous.len(), 2);
    let changing = vec![ap("aabbccddeeff", &[("gid", H)])];
    assert!(!topology(&changing)
        .candidate_sets
        .contains_key(&format!("gid:{G}")));
}
#[test]
fn info_projection_preserves_formats_conflicts_and_excludes_secrets() {
    let mut d = plist::Dictionary::new();
    d.insert(
        "features".into(),
        Value::Integer(((1u64 << 46) | (1 << 19) | (1 << 9)).into()),
    );
    d.insert("shk".into(), Value::Data(b"SECRET-KEY".to_vec()));
    d.insert(
        "supportedFormats".into(),
        super::super::profile::dict([
            ("audioStream", Value::Integer(0x40000u64.into())),
            ("bufferStream", Value::Integer((1u64 << 23).into())),
        ]),
    );
    let value = Value::Dictionary(d.clone());
    let info = Evidence::authenticated_info(&value);
    assert_eq!(info.buffered_formats, Claim::Observed([23].into()));
    assert!(!format!("{info:?}").contains("SECRET"));
    let r = ap("aabbccddeeff", &[]);
    let t = topology(std::slice::from_ref(&r));
    assert!(plan(&r, authorized(), &t, Some(&info))[0]
        .rejections
        .is_empty());
    d.insert(
        "supportedAudioFormatsExtended".into(),
        super::super::profile::dict([(
            "bufferStream",
            Value::Array(vec![Value::Integer(22u64.into())]),
        )]),
    );
    assert_eq!(
        Evidence::authenticated_info(&Value::Dictionary(d.clone())).buffered_formats,
        Claim::Conflict
    );
    for invalid in [
        Value::String("bad".into()),
        Value::Integer(5u64.into()),
        Value::Array(vec![Value::String("23".into())]),
    ] {
        d.insert(
            "supportedAudioFormatsExtended".into(),
            super::super::profile::dict([("bufferStream", invalid)]),
        );
        assert_eq!(
            Evidence::authenticated_info(&Value::Dictionary(d.clone())).buffered_formats,
            Claim::Invalid
        );
    }
    d.insert("features".into(), Value::Integer(0u64.into()));
    let conflict = Evidence::authenticated_info(&Value::Dictionary(d.clone()));
    assert!(plan(&r, authorized(), &t, Some(&conflict))
        .iter()
        .all(|c| !c.rejections.is_empty()));
    d.insert("statusFlags".into(), Value::Integer(128u64.into()));
    assert!(plan(
        &r,
        authorized(),
        &t,
        Some(&Evidence::authenticated_info(&Value::Dictionary(d)))
    )
    .iter()
    .all(|c| !c.rejections.is_empty()));
}
#[test]
fn diagnostic_is_deterministic_and_redacted() {
    let mut r = ap("aabbccddeeff", &[("gid", "invalid"), ("igl", "maybe")]);
    r.services[0]
        .capabilities
        .insert("pk".into(), "SECRET".into());
    let text = diagnostic(&[r], |_| false, None);
    assert!(text.contains("not authorized"));
    assert!(!text.contains("SECRET"));
    assert!(text.contains("audible_playback: None"));
}

#[test]
fn passive_dispatch_never_connects_or_mutates_state() {
    use clap::Parser;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let source = "receiver_allowlist=['device:aabbccddeeff']\n";
    std::fs::write(&config, source).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    for method in [None, Some("setup-code"), Some("transient")] {
        let mut args = vec![
            "piair",
            "--config",
            config.to_str().unwrap(),
            "receiver-plan",
        ];
        if let Some(method) = method {
            args.extend(["--method", method]);
        }
        let cli = crate::cli::Cli::parse_from(args);
        crate::run_with_discovery(cli, |_| {
            let mut r = ap("aabbccddeeff", &[]);
            r.services[0].port = port;
            Ok(vec![r])
        })
        .unwrap();
        assert!(listener.accept().is_err());
        assert_eq!(std::fs::read_to_string(&config).unwrap(), source);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn labels_cannot_grant_capabilities_and_split_gids_stay_separate() {
    let mut r = ap(
        "aabbccddeeff",
        &[
            ("model", "HomePod"),
            ("tsid", H),
            ("gid", &format!("{H}+0")),
        ],
    );
    let other = ap("112233445566", &[("tsid", H), ("gid", &format!("{H}+1"))]);
    r.name = "Apple TV".into();
    let topology = topology(&[r, other]);
    assert_eq!(topology.stereo_candidates[&format!("tsid:{H}")].len(), 2);
    assert_eq!(topology.candidate_sets[&format!("gid:{H}+0")].len(), 1);
    assert_eq!(topology.candidate_sets[&format!("gid:{H}+1")].len(), 1);
    assert!(topology.proxy_candidates.is_empty());
    let mut unknown = receiver(
        "aabbccddeeff",
        "_airplay._tcp.local.",
        &[("model", "AppleTV")],
    );
    unknown.name = "HomePod".into();
    let caps = ReceiverCapabilities::observe(&unknown);
    assert_eq!(caps.alac, Claim::Unknown);
    assert_eq!(caps.buffered, Claim::Unknown);
    assert_eq!(caps.ptp, Claim::Unknown);
}

#[test]
fn malformed_info_parents_and_status_are_never_silently_promoted() {
    for value in [
        Value::String("bad".into()),
        super::super::profile::dict([("supportedFormats", Value::String("bad".into()))]),
        super::super::profile::dict([(
            "supportedAudioFormatsExtended",
            Value::String("bad".into()),
        )]),
    ] {
        let info = Evidence::authenticated_info(&value);
        assert!(info.realtime_format == Claim::Invalid || info.buffered_formats == Claim::Invalid);
    }
    let r = receiver(
        "aabbccddeeff",
        "_airplay._tcp.local.",
        &[("features", "0x80200,0x10000"), ("flags", "0")],
    );
    let t = topology(std::slice::from_ref(&r));
    let auth = Authorization {
        method: Some(Method::Transient),
        ..authorized()
    };
    assert!(plan(&r, auth, &t, None)[0].rejections.is_empty());
    for flags in [1u64 << 3, 1 << 7, 1 << 9] {
        let info = Evidence::authenticated_info(&super::super::profile::dict([(
            "statusFlags",
            Value::Integer(flags.into()),
        )]));
        assert!(plan(&r, auth, &t, Some(&info))
            .iter()
            .all(|c| !c.rejections.is_empty()));
    }
}

#[test]
fn existing_local_state_is_never_read_or_changed_by_diagnostics() {
    use clap::Parser;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "receiver_allowlist=['device:aabbccddeeff']").unwrap();
    let state = dir.path().join("receiver-state");
    std::fs::create_dir(&state).unwrap();
    // Deliberately invalid local state proves dispatch does not parse credentials/selection.
    let selected = state.join("selected.toml");
    let credentials = state.join("aabbccddeeff.toml");
    for path in [&selected, &credentials] {
        std::fs::write(path, "SECRET invalid local material").unwrap();
    }
    let cli = crate::cli::Cli::parse_from([
        "piair",
        "--config",
        config.to_str().unwrap(),
        "receiver-plan",
    ]);
    crate::run_with_discovery(cli, |_| Ok(vec![ap("aabbccddeeff", &[])])).unwrap();
    for path in [&selected, &credentials] {
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "SECRET invalid local material"
        );
    }
    assert_eq!(std::fs::read_dir(state).unwrap().count(), 2);
}

#[test]
fn unknown_extended_format_ids_remain_evidence_without_enabling_streaming() {
    let value = super::super::profile::dict([(
        "supportedAudioFormatsExtended",
        super::super::profile::dict([(
            "bufferStream",
            Value::Array(vec![Value::Integer(99u64.into())]),
        )]),
    )]);
    let evidence = Evidence::authenticated_info(&value);
    assert_eq!(evidence.buffered_formats, Claim::Observed([99].into()));
    assert!(evidence.raw["supportedAudioFormatsExtended.bufferStream"].contains("99"));
    let r = ap("aabbccddeeff", &[]);
    let t = topology(std::slice::from_ref(&r));
    let candidates = plan(&r, authorized(), &t, Some(&evidence));
    assert!(candidates
        .iter()
        .filter(|c| c.transport == Transport::BufferedPtp)
        .all(|c| c.implementation == Implementation::Unsupported && !c.rejections.is_empty()));
}

fn buffered_receiver() -> Receiver {
    // Buffered, PTP and HAP only: deliberately no realtime audio/ALAC bits.
    receiver(
        "aabbccddeeff",
        "_airplay._tcp.local.",
        &[
            ("features", "0x0,0x4300"),
            ("flags", "0"),
            ("sr", "48000"),
            ("ss", "24"),
            ("ch", "6"),
        ],
    )
}
fn buffered_info(ids: &[u64]) -> Evidence {
    Evidence::authenticated_info(&super::super::profile::dict([(
        "supportedAudioFormatsExtended",
        super::super::profile::dict([(
            "bufferStream",
            Value::Array(ids.iter().map(|id| Value::Integer((*id).into())).collect()),
        )]),
    )]))
}
fn candidate(candidates: &[Candidate], transport: Transport) -> &Candidate {
    candidates
        .iter()
        .find(|c| c.transport == transport)
        .unwrap()
}

#[test]
fn buffered_formats_and_codecs_are_independent_of_realtime_alac() {
    let r = buffered_receiver();
    let t = topology(std::slice::from_ref(&r));
    for (id, expected) in [
        (22, BufferedFormat::AacLc44100Stereo),
        (23, BufferedFormat::AacLc48000Stereo),
        (21, BufferedFormat::Alac48000Stereo24),
        (39, BufferedFormat::AacLc48000Surround),
        (99, BufferedFormat::Unknown(99)),
    ] {
        let mut info = buffered_info(&[id]);
        // Conflicting realtime format evidence has no bearing on buffered AAC.
        info.realtime_format = Claim::Invalid;
        let candidates = plan(&r, authorized(), &t, Some(&info));
        let buffered = candidate(&candidates, Transport::BufferedPtp);
        assert_eq!(
            buffered.media,
            MediaProfile::Buffered(Claim::Observed(vec![expected]))
        );
        assert!(
            buffered
                .rejections
                .iter()
                .all(|reason| !reason.contains("audio/ALAC claims")
                    && !reason.contains("sample format")
                    && !reason.contains("PTP claim")
                    && !reason.contains("buffered audio claim")),
            "{buffered:?}"
        );
        assert_eq!(buffered.implementation, Implementation::Unsupported);
        assert!(!buffered.rejections.is_empty());
        assert_ne!(expected.codec_available(), Claim::Observed(true));
        assert!(buffered
            .rejections
            .iter()
            .any(|r| r.contains("codec/format")));
        assert!(candidate(&candidates, Transport::RealtimeNtp)
            .rejections
            .iter()
            .any(|r| r.contains("audio/ALAC claims")));
    }
}

#[test]
fn buffered_evidence_is_unknown_until_authenticated_and_conflicts_fail_closed() {
    let r = buffered_receiver();
    let t = topology(std::slice::from_ref(&r));
    let missing = Evidence::authenticated_info(&Value::Dictionary(Default::default()));
    let mut untrusted = buffered_info(&[23]);
    untrusted.provenance = Provenance::Mdns {
        service: "untrusted".into(),
    };
    for info in [None, Some(&missing), Some(&untrusted)] {
        let candidates = plan(&r, authorized(), &t, info);
        let buffered = candidate(&candidates, Transport::BufferedPtp);
        assert_eq!(buffered.media, MediaProfile::Buffered(Claim::Unknown));
        assert!(buffered
            .rejections
            .iter()
            .any(|r| r.contains("format evidence unknown")));
    }
    let conflict = Evidence::authenticated_info(&super::super::profile::dict([
        (
            "supportedFormats",
            super::super::profile::dict([("bufferStream", Value::Integer((1u64 << 22).into()))]),
        ),
        (
            "supportedAudioFormatsExtended",
            super::super::profile::dict([(
                "bufferStream",
                Value::Array(vec![Value::Integer(23u64.into())]),
            )]),
        ),
    ]));
    for formats in [Claim::Conflict, Claim::Invalid] {
        let mut info = conflict.clone();
        info.buffered_formats = formats.clone();
        let candidates = plan(&r, authorized(), &t, Some(&info));
        let buffered = candidate(&candidates, Transport::BufferedPtp);
        let expected = match formats {
            Claim::Conflict => Claim::Conflict,
            _ => Claim::Invalid,
        };
        assert_eq!(buffered.media, MediaProfile::Buffered(expected));
        assert!(buffered
            .rejections
            .iter()
            .any(|r| r.contains("buffered format claims malformed or contradictory")));
    }
    assert_eq!(conflict.buffered_formats, Claim::Conflict);
    let empty = buffered_info(&[]);
    assert!(candidate(
        &plan(&r, authorized(), &t, Some(&empty)),
        Transport::BufferedPtp
    )
    .rejections
    .iter()
    .any(|r| r.contains("offers no buffered formats")));
}

#[test]
fn profile_specific_feature_and_format_changes_do_not_leak() {
    let r = ap("aabbccddeeff", &[]);
    let t = topology(std::slice::from_ref(&r));
    let before = plan(&r, authorized(), &t, None);
    let mut unrelated = buffered_info(&[99]);
    unrelated.features = Claim::Observed((1u64 << 46) | (1 << 19) | (1 << 9) | (1 << 40));
    unrelated.buffered_formats = Claim::Conflict;
    let after = plan(&r, authorized(), &t, Some(&unrelated));
    assert_eq!(
        candidate(&before, Transport::RealtimeNtp),
        candidate(&after, Transport::RealtimeNtp)
    );
    assert_eq!(
        candidate(&before, Transport::RealtimePtp),
        candidate(&after, Transport::RealtimePtp)
    );
    unrelated.realtime_format = Claim::Observed(0);
    for transport in [Transport::RealtimeNtp, Transport::RealtimePtp] {
        assert!(
            candidate(&plan(&r, authorized(), &t, Some(&unrelated)), transport)
                .rejections
                .iter()
                .any(|r| r.contains("excludes realtime ALAC"))
        );
    }
    // Realtime ALAC changes in authenticated features do not gate buffered AAC.
    let r = buffered_receiver();
    let t = topology(std::slice::from_ref(&r));
    let mut info = buffered_info(&[23]);
    info.features = Claim::Observed((1u64 << 46) | (1 << 40) | (1 << 41));
    let before = plan(&r, authorized(), &t, Some(&info));
    info.features = Claim::Observed((1u64 << 46) | (1 << 40) | (1 << 41) | (1 << 19) | (1 << 9));
    let after = plan(&r, authorized(), &t, Some(&info));
    assert_eq!(
        candidate(&before, Transport::BufferedPtp),
        candidate(&after, Transport::BufferedPtp)
    );
}

#[test]
fn raop_and_airplay_service_requirements_do_not_leak_across_profiles() {
    let mut r = ap("aabbccddeeff", &[]);
    let raop = receiver(
        "aabbccddeeff",
        "_raop._tcp.local.",
        &[
            ("features", "bad"),
            ("cn", "0"),
            ("sr", "48000"),
            ("ss", "24"),
            ("ch", "1"),
        ],
    );
    let before = plan(&r, authorized(), &topology(std::slice::from_ref(&r)), None);
    r.services.extend(raop.services);
    let t = topology(std::slice::from_ref(&r));
    let after = plan(&r, authorized(), &t, None);
    assert_eq!(
        candidate(&before, Transport::RealtimeNtp),
        candidate(&after, Transport::RealtimeNtp)
    );
    let raop_before = candidate(&after, Transport::Raop).clone();
    let bad_info = Evidence::authenticated_info(&Value::String("bad AirPlay 2 info".into()));
    let after = plan(
        &r,
        Authorization {
            method: None,
            ..authorized()
        },
        &t,
        Some(&bad_info),
    );
    assert_eq!(candidate(&after, Transport::Raop), &raop_before);
    assert_eq!(raop_before.media, MediaProfile::RaopUnresolved);
    assert!(raop_before
        .rejections
        .iter()
        .any(|r| r.contains("RAOP codec, authentication and timing requirements unresolved")));
}

#[test]
fn shared_authorization_still_blocks_every_profile_and_no_transport_is_enabled() {
    let r = buffered_receiver();
    let t = topology(std::slice::from_ref(&r));
    let info = buffered_info(&[23]);
    for authorization in [
        Authorization::default(),
        Authorization {
            allowlisted: false,
            ..authorized()
        },
    ] {
        for c in plan(&r, authorization, &t, Some(&info)) {
            assert!(c
                .rejections
                .iter()
                .any(|r| r.contains("not authorized by allowlist")));
        }
    }
    for c in plan(&r, authorized(), &t, Some(&info)) {
        if c.transport != Transport::RealtimeNtp {
            assert_eq!(c.implementation, Implementation::Unsupported);
            assert!(c
                .rejections
                .iter()
                .any(|r| r.contains("transport is not implemented")));
        }
    }
    let mut restricted = r;
    restricted.services[0]
        .capabilities
        .insert("pw".into(), "true".into());
    for c in plan(&restricted, authorized(), &t, Some(&info))
        .iter()
        .filter(|c| c.transport != Transport::Raop)
    {
        assert!(c
            .rejections
            .iter()
            .any(|r| r.contains("password-protected")));
    }
}
