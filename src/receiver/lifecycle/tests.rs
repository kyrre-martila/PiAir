use super::*;
use crate::receiver::{crypto::ControlKeys, wire::Wire};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream, UdpSocket},
    thread,
};

fn info() -> Value {
    profile::dict([
        ("deviceID", Value::String("11:22:33:44:55:66".into())),
        ("features", profile::uint((1u64 << 40) | (1u64 << 41))),
        ("statusFlags", profile::uint(0)),
        (
            "supportedFormats",
            profile::dict([("bufferStream", profile::uint(1 << 21))]),
        ),
    ])
}
fn setup(clock: u64) -> Value {
    profile::dict([
        ("eventPort", profile::uint(12345)),
        ("timingProtocol", Value::String("PTP".into())),
        ("timingPort", profile::uint(0)),
        (
            "timingPeerInfo",
            profile::dict([
                ("ClockID", Value::Integer((clock as i64).into())),
                (
                    "Addresses",
                    Value::Array(vec![Value::String("127.0.0.2".into())]),
                ),
            ]),
        ),
    ])
}
fn read_request(socket: &mut TcpStream, keys: &mut ControlKeys) -> (String, Vec<u8>) {
    let mut bytes = Vec::new();
    loop {
        let mut len = [0; 2];
        socket.read_exact(&mut len).unwrap();
        let size = usize::from(u16::from_le_bytes(len));
        assert!((1..=1024).contains(&size));
        let mut frame = vec![0; size + 18];
        frame[..2].copy_from_slice(&len);
        socket.read_exact(&mut frame[2..]).unwrap();
        bytes.extend_from_slice(&keys.decrypt(&frame).unwrap());
        if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            let header = String::from_utf8(bytes[..end + 4].to_vec()).unwrap();
            let length = header
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            if bytes.len() == end + 4 + length {
                return (header, bytes[end + 4..].to_vec());
            }
        }
        assert!(bytes.len() < 16384);
    }
}
fn reply(socket: &mut TcpStream, keys: &mut ControlKeys, seq: u32, value: Option<Value>) {
    let body = value
        .map(|mut v| profile::binary(&mut v).unwrap())
        .unwrap_or_default();
    let mut response = format!("RTSP/1.0 200 OK\r\nCSeq: {seq}\r\nContent-Type: application/x-apple-binary-plist\r\nSession: test-session\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    response.extend_from_slice(&body);
    for part in response.chunks(31) {
        socket.write_all(&keys.encrypt(part).unwrap()).unwrap();
    }
}
fn fixture(
    server: impl FnOnce(TcpStream) + Send + 'static,
) -> (Session, tempfile::TempDir, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.2:0").unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let handle = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        server(socket);
    });
    let mut wire = Wire::mock(socket, Duration::from_millis(500), "device:112233445566");
    let authorization = wire.ptp_test_authorization().unwrap();
    let wire = wire
        .secure(ControlKeys::new(b"lifecycle"), ControlKeys::new(b"events"))
        .unwrap();
    let mut owner = Session::new();
    owner.control = Some(wire);
    owner.state.advance(State::Authentication).unwrap();
    owner.state.advance(State::Negotiation).unwrap();
    (owner, authorization, handle)
}
fn server_keys() -> ControlKeys {
    let mut keys = ControlKeys::new(b"lifecycle");
    keys.swap_directions_for_test();
    keys
}
fn udp_pair() -> (UdpSocket, UdpSocket) {
    let local = UdpSocket::bind("127.0.0.1:0").unwrap();
    let peer = UdpSocket::bind("127.0.0.2:0").unwrap();
    local.connect(peer.local_addr().unwrap()).unwrap();
    peer.connect(local.local_addr().unwrap()).unwrap();
    (local, peer)
}

#[test]
fn state_machine_has_no_path_to_playing_and_rejects_duplicate_or_stale_operations() {
    let states = [
        State::Authorization,
        State::Authentication,
        State::Negotiation,
        State::Timing,
        State::Prebuffering,
        State::Playing,
        State::Paused,
        State::Stopping,
        State::Failed,
        State::Closed,
    ];
    for from in states {
        for to in [State::Prebuffering, State::Playing, State::Paused] {
            let mut state = from;
            assert!(state.advance(to).is_err());
            assert_eq!(state, from);
        }
    }
    let mut owner = Session::new();
    owner.stop().unwrap();
    assert_eq!(owner.state(), State::Closed);
    assert!(owner.stop().is_err());
    assert!(owner
        .start(|| panic!("stale session must not connect"), None)
        .is_err());
    assert!(owner.observe(Duration::from_millis(1)).is_err());
    let mut owner = Session::new();
    owner.cancel.store(true, Ordering::Relaxed);
    assert!(owner
        .start(|| panic!("cancelled session must not connect"), None)
        .is_err());
    assert_eq!(owner.state(), State::Failed);
    assert!(owner.request_playback().is_err());
}
#[test]
fn authenticated_info_checks_identity_codec_policy_and_conflicting_evidence() {
    assert!(validate_info(&info(), "device:112233445566").is_ok());
    assert!(validate_info(&info(), "device:223344556677").is_err());
    for (field, value) in [
        ("deviceID", Value::String("bad".into())),
        ("features", profile::uint(1 << 40)),
        ("statusFlags", profile::uint(1 << 7)),
        (
            "supportedFormats",
            profile::dict([("bufferStream", profile::uint(1 << 23))]),
        ),
        (
            "supportedAudioFormatsExtended",
            profile::dict([("bufferStream", Value::Array(vec![profile::uint(23)]))]),
        ),
        ("isGroupLeader", Value::Boolean(true)),
        ("act", profile::uint(1)),
    ] {
        let mut value_info = info();
        value_info
            .as_dictionary_mut()
            .unwrap()
            .insert(field.into(), value);
        assert!(
            validate_info(&value_info, "device:112233445566").is_err(),
            "{field}"
        );
    }
}
#[test]
fn initial_setup_pins_clock_address_protocol_and_ports() {
    let peer = "127.0.0.2".parse().unwrap();
    let good = setup(0xf122334455667788);
    assert_eq!(
        InitialSetup::parse(&good, peer, 42).unwrap().clock_id,
        0xf122334455667788
    );
    for clock in [0, u64::MAX, 42] {
        assert!(InitialSetup::parse(&setup(clock), peer, 42).is_err());
    }
    for (field, value) in [
        ("eventPort", profile::uint(0)),
        ("eventPort", profile::uint(65536)),
        ("timingPort", profile::uint(319)),
        ("timingProtocol", Value::String("NTP".into())),
        ("streams", Value::Array(vec![])),
        ("status", profile::uint(1)),
        ("timingPeerList", Value::Array(vec![])),
    ] {
        let mut bad = good.clone();
        bad.as_dictionary_mut().unwrap().insert(field.into(), value);
        assert!(InitialSetup::parse(&bad, peer, 42).is_err());
    }
    for (field, value) in [
        ("ClockID", Value::String("123".into())),
        (
            "Addresses",
            Value::Array(vec![Value::String("127.0.0.3".into())]),
        ),
        (
            "Addresses",
            Value::Array(vec![
                Value::String("127.0.0.2".into()),
                Value::String("127.0.0.3".into()),
            ]),
        ),
    ] {
        let mut bad = good.clone();
        bad.as_dictionary_mut()
            .unwrap()
            .get_mut("timingPeerInfo")
            .unwrap()
            .as_dictionary_mut()
            .unwrap()
            .insert(field.into(), value);
        assert!(InitialSetup::parse(&bad, peer, 42).is_err());
    }
    let mut bad = good;
    bad.as_dictionary_mut()
        .unwrap()
        .get_mut("timingPeerInfo")
        .unwrap()
        .as_dictionary_mut()
        .unwrap()
        .remove("ClockID");
    assert!(InitialSetup::parse(&bad, peer, 42).is_err());
}
#[test]
fn encrypted_initial_lifecycle_observes_peer_without_ever_granting_playback() {
    let (mut owner, _approval, server) = fixture(|mut socket| {
        let mut keys = server_keys();
        let (head, _) = read_request(&mut socket, &mut keys);
        assert!(head.starts_with("GET /info "));
        reply(&mut socket, &mut keys, 1, Some(info()));
        let (head, body) = read_request(&mut socket, &mut keys);
        assert!(head.starts_with("SETUP rtsp://"));
        let offer = profile::parse(&body).unwrap();
        assert_eq!(
            profile::dictionary(&offer).unwrap()["timingProtocol"].as_string(),
            Some("PTP")
        );
        assert!(!profile::dictionary(&offer).unwrap().contains_key("streams"));
        reply(&mut socket, &mut keys, 2, Some(setup(123)));
        let (head, _) = read_request(&mut socket, &mut keys);
        assert!(head.starts_with("TEARDOWN "));
        assert!(head.contains("Session: test-session\r\n"));
        reply(&mut socket, &mut keys, 3, None);
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    });
    let (event, peer_event) = udp_pair();
    let (general, _peer_general) = udp_pair();
    let local_event = event.local_addr().unwrap();
    let local_general = general.local_addr().unwrap();
    owner
        .negotiate_with(|wire, setup, clock| {
            let clock = ptp::Grandmaster::new(
                clock,
                setup.clock_id,
                Instant::now(),
                Duration::from_secs(2),
            )?;
            Ok((
                None,
                ptp::Service::attach(clock, event, general, wire.ptp_lease()?)?,
            ))
        })
        .unwrap();
    assert_eq!(owner.state(), State::Timing);
    let packet = ptp::packet::Packet::new(ptp::packet::Kind::DelayReq, 123, 1, vec![0; 10])
        .encode()
        .unwrap();
    peer_event.send(&packet).unwrap();
    assert_eq!(
        owner.observe(Duration::from_millis(1)).unwrap(),
        ptp::State::PeerObserved
    );
    assert!(owner.observe(Duration::from_millis(1)).is_err());
    assert!(!owner.timing.as_ref().unwrap().clock().media_permitted());
    assert!(owner
        .request_playback()
        .unwrap_err()
        .to_string()
        .contains("clock adoption"));
    assert_eq!(owner.state(), State::Failed);
    assert!(owner.control.is_none());
    assert!(owner.timing.is_none());
    UdpSocket::bind(local_event).unwrap();
    UdpSocket::bind(local_general).unwrap();
    server.join().unwrap();
}
#[test]
fn partial_setup_faults_attempt_teardown_and_release_all_resources() {
    // Inject at /info validation, initial SETUP validation and resource attachment.
    for fault in 0..3 {
        let (mut owner, _approval, server) = fixture(move |mut socket| {
            let mut keys = server_keys();
            read_request(&mut socket, &mut keys);
            let value = if fault == 0 {
                profile::dict([])
            } else {
                info()
            };
            reply(&mut socket, &mut keys, 1, Some(value));
            if fault == 0 {
                let mut b = [0];
                assert_eq!(socket.read(&mut b).unwrap(), 0);
                return;
            }
            read_request(&mut socket, &mut keys);
            reply(
                &mut socket,
                &mut keys,
                2,
                Some(if fault == 1 {
                    profile::dict([])
                } else {
                    setup(123)
                }),
            );
            let (head, _) = read_request(&mut socket, &mut keys);
            assert!(head.starts_with("TEARDOWN "));
            reply(&mut socket, &mut keys, 3, None);
        });
        let result = owner.negotiate_with(|_, _, _| bail!("injected resource failure"));
        assert!(owner.finish(result).is_err());
        assert_eq!(owner.state(), State::Failed);
        assert!(owner.control.is_none());
        assert!(owner.events.is_none());
        assert!(owner.timing.is_none());
        server.join().unwrap();
    }
}
#[test]
fn fresh_restart_never_reuses_clock_or_session_identity() {
    let first = Session::new();
    let second = Session::new();
    assert_ne!(first.clock_id, second.clock_id);
    assert_ne!(first.uuid, second.uuid);
}

fn attach_test_timing(
    wire: &mut Wire,
    setup: InitialSetup,
    id: u64,
) -> Result<(Option<Worker>, ptp::Service)> {
    let (event, _peer_event) = udp_pair();
    let (general, _peer_general) = udp_pair();
    Ok((
        None,
        ptp::Service::attach(
            ptp::Grandmaster::new(id, setup.clock_id, Instant::now(), Duration::from_secs(2))?,
            event,
            general,
            wire.ptp_lease()?,
        )?,
    ))
}
fn setup_server(mut socket: TcpStream, acknowledge_teardown: bool) {
    let mut keys = server_keys();
    read_request(&mut socket, &mut keys);
    reply(&mut socket, &mut keys, 1, Some(info()));
    read_request(&mut socket, &mut keys);
    reply(&mut socket, &mut keys, 2, Some(setup(123)));
    if acknowledge_teardown {
        let (head, _) = read_request(&mut socket, &mut keys);
        assert!(head.starts_with("TEARDOWN "));
        reply(&mut socket, &mut keys, 3, None);
    } else {
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    }
}
#[test]
fn cancellation_clock_loss_and_drop_attempt_authenticated_teardown() {
    for fault in 0..3 {
        let (mut owner, _approval, server) = fixture(|socket| setup_server(socket, true));
        owner.negotiate_with(attach_test_timing).unwrap();
        match fault {
            0 => {
                owner.cancel.store(true, Ordering::Relaxed);
                assert!(owner.observe(Duration::from_millis(1)).is_err());
                assert_eq!(owner.state(), State::Failed);
                assert!(owner.control.is_none());
            }
            1 => {
                owner.timing.as_mut().unwrap().clock_mut().shutdown();
                assert!(owner.observe(Duration::from_millis(1)).is_err());
                assert!(owner.timing.is_none());
            }
            _ => (),
        }
        drop(owner);
        server.join().unwrap();
    }
}
#[test]
fn revocation_prevents_timing_and_teardown_network_writes() {
    let (mut owner, approval, server) = fixture(|socket| setup_server(socket, false));
    owner.negotiate_with(attach_test_timing).unwrap();
    std::fs::write(approval.path().join("config.toml"), "receiver_allowlist=[]").unwrap();
    let error = owner.observe(Duration::from_millis(1)).unwrap_err();
    assert!(format!("{error:#}").contains("revoked"));
    assert!(owner.control.is_none());
    assert!(owner.timing.is_none());
    server.join().unwrap();
}
#[test]
fn teardown_stall_is_bounded_and_does_not_leave_a_control_socket() {
    let (release, wait) = std::sync::mpsc::channel();
    let (mut owner, _approval, server) = fixture(move |mut socket| {
        let mut keys = server_keys();
        read_request(&mut socket, &mut keys);
        reply(&mut socket, &mut keys, 1, Some(info()));
        read_request(&mut socket, &mut keys);
        reply(&mut socket, &mut keys, 2, Some(setup(123)));
        let (head, _) = read_request(&mut socket, &mut keys);
        assert!(head.starts_with("TEARDOWN "));
        wait.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut b = [0];
        assert_eq!(socket.read(&mut b).unwrap(), 0);
    });
    owner.negotiate_with(attach_test_timing).unwrap();
    assert!(owner.stop().is_err());
    assert_eq!(owner.state(), State::Failed);
    assert!(owner.control.is_none());
    assert!(owner.timing.is_none());
    release.send(()).unwrap();
    server.join().unwrap();
}
#[test]
fn disconnect_during_setup_closes_without_reusing_cipher_or_retrying() {
    let (mut owner, _approval, server) = fixture(|mut socket| {
        let mut keys = server_keys();
        read_request(&mut socket, &mut keys);
        reply(&mut socket, &mut keys, 1, Some(info()));
        let (head, _) = read_request(&mut socket, &mut keys);
        assert!(head.starts_with("SETUP "));
    });
    let result = owner.negotiate_with(attach_test_timing);
    let error = owner.finish(result).unwrap_err();
    assert!(format!("{error:#}").contains("control invalidated"));
    assert_eq!(owner.state(), State::Failed);
    assert!(owner.control.is_none());
    assert!(owner
        .start(|| panic!("failed owner must not retry"), None)
        .is_err());
    server.join().unwrap();
}

#[test]
fn authorization_is_mandatory_before_authentication_or_any_control_request() {
    let listener = TcpListener::bind("127.0.0.2:0").unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let mut peer = listener.accept().unwrap().0;
    peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let wire = Wire::mock(socket, Duration::from_millis(100), "device:112233445566");
    let mut owner = Session::new();
    assert!(owner
        .start(|| Ok(wire), None)
        .unwrap_err()
        .to_string()
        .contains("authorization"));
    assert_eq!(owner.state(), State::Failed);
    let mut byte = [0];
    assert_eq!(peer.read(&mut byte).unwrap(), 0);
}

#[test]
fn control_stages_validate_order_payloads_and_never_connect_media() {
    for target in [Stage::Record, Stage::Stream, Stage::Anchor] {
        let media = TcpListener::bind("127.0.0.2:0").unwrap();
        media.set_nonblocking(true).unwrap();
        let port = media.local_addr().unwrap().port();
        let (mut owner, _approval, server) = fixture(move |mut socket| {
            let mut keys = server_keys();
            read_request(&mut socket, &mut keys);
            reply(&mut socket, &mut keys, 1, Some(info()));
            read_request(&mut socket, &mut keys);
            reply(&mut socket, &mut keys, 2, Some(setup(123)));
            let (head, body) = read_request(&mut socket, &mut keys);
            assert!(head.starts_with("RECORD "));
            assert!(body.is_empty());
            // PTP must continue while this response is pending.
            thread::sleep(Duration::from_millis(160));
            reply(&mut socket, &mut keys, 3, None);
            let mut next = 4;
            if target >= Stage::Stream {
                let (head, body) = read_request(&mut socket, &mut keys);
                assert!(head.starts_with("SETPEERS "));
                assert!(head.contains("Content-Type: /peer-list-changed\r\n"));
                assert_eq!(
                    profile::parse(&body).unwrap(),
                    Value::Array(vec!["127.0.0.2".into(), "127.0.0.1".into()])
                );
                reply(&mut socket, &mut keys, 4, None);
                let (head, body) = read_request(&mut socket, &mut keys);
                assert!(head.starts_with("POST /command "));
                let value = profile::parse(&body).unwrap();
                let value = value.as_dictionary().unwrap();
                assert_eq!(value["type"].as_string(), Some("updateMRSupportedCommands"));
                assert!(
                    value["params"].as_dictionary().unwrap()["mrSupportedCommandsFromSender"]
                        .as_array()
                        .unwrap()
                        .is_empty()
                );
                reply(
                    &mut socket,
                    &mut keys,
                    5,
                    Some(profile::dict([("status", profile::uint(0))])),
                );
                let (head, body) = read_request(&mut socket, &mut keys);
                assert!(head.starts_with("SETUP "));
                let value = profile::parse(&body).unwrap();
                let stream = value.as_dictionary().unwrap()["streams"]
                    .as_array()
                    .unwrap()[0]
                    .as_dictionary()
                    .unwrap();
                assert_eq!(stream["type"].as_unsigned_integer(), Some(103));
                assert_eq!(stream["audioFormatIndex"].as_unsigned_integer(), Some(21));
                assert_eq!(stream["shk"].as_data().unwrap().len(), 32);
                assert_eq!(
                    stream["streamConnections"].as_dictionary().unwrap()["streamConnectionTypeRTP"]
                        .as_dictionary()
                        .unwrap()["streamConnectionKeyUseStreamEncryptionKey"]
                        .as_boolean(),
                    Some(true)
                );
                let response = profile::dict([(
                    "streams",
                    Value::Array(vec![profile::dict([
                        ("type", profile::uint(103)),
                        ("streamConnectionID", stream["streamConnectionID"].clone()),
                        (
                            "streamConnections",
                            profile::dict([(
                                "streamConnectionTypeRTP",
                                profile::dict([(
                                    "streamConnectionKeyPort",
                                    profile::uint(u64::from(port)),
                                )]),
                            )]),
                        ),
                    ])]),
                )]);
                reply(&mut socket, &mut keys, 6, Some(response));
                let (head, body) = read_request(&mut socket, &mut keys);
                assert!(head.starts_with("POST /audioMode "));
                assert_eq!(
                    profile::parse(&body).unwrap(),
                    profile::dict([("audioMode", "default".into())])
                );
                reply(&mut socket, &mut keys, 7, None);
                next = 8;
            }
            if target == Stage::Anchor {
                let (head, body) = read_request(&mut socket, &mut keys);
                assert!(head.starts_with("SETRATEANCHORTIME "));
                let value = profile::parse(&body).unwrap();
                assert_eq!(
                    value.as_dictionary().unwrap()["rate"].as_unsigned_integer(),
                    Some(0)
                );
                assert_eq!(
                    value.as_dictionary().unwrap()["rtpTime"].as_unsigned_integer(),
                    Some(0)
                );
                reply(&mut socket, &mut keys, 8, Some(value));
                next = 9;
            }
            let (head, _) = read_request(&mut socket, &mut keys);
            assert!(head.starts_with("TEARDOWN "));
            assert!(head.contains("Session: test-session\r\n"));
            reply(&mut socket, &mut keys, next, None);
        });
        let (event, peer_event) = udp_pair();
        let (general, _peer_general) = udp_pair();
        owner
            .negotiate_with(|wire, setup, id| {
                let clock = ptp::Grandmaster::new(
                    id,
                    setup.clock_id,
                    Instant::now(),
                    Duration::from_secs(30),
                )?;
                Ok((
                    None,
                    ptp::Service::attach(clock, event, general, wire.ptp_lease()?)?,
                ))
            })
            .unwrap();
        peer_event
            .send(
                &ptp::packet::Packet::new(ptp::packet::Kind::DelayReq, 123, 1, vec![0; 10])
                    .encode()
                    .unwrap(),
            )
            .unwrap();
        owner.observe(Duration::from_millis(1)).unwrap();
        owner.progress(target).unwrap();
        assert_eq!(owner.stage, Some(target));
        assert_eq!(owner.anchor_accepted, target == Stage::Anchor);
        assert!(!owner.timing.as_ref().unwrap().clock().media_permitted());
        // Initial Sync and at least one more while the RECORD reply stalled.
        peer_event.set_nonblocking(true).unwrap();
        let mut count = 0;
        let mut bytes = [0; 512];
        while peer_event.recv(&mut bytes).is_ok() {
            count += 1;
        }
        assert!(count >= 2);
        assert!(media.accept().is_err());
        owner.stop().unwrap();
        assert!(owner.prepared.is_none());
        assert!(owner.stream.is_none());
        server.join().unwrap();
    }
}

#[test]
fn rejected_record_never_advances_and_attempts_teardown() {
    let (mut owner, _approval, server) = fixture(|mut socket| {
        let mut keys = server_keys();
        read_request(&mut socket, &mut keys);
        reply(&mut socket, &mut keys, 1, Some(info()));
        read_request(&mut socket, &mut keys);
        reply(&mut socket, &mut keys, 2, Some(setup(123)));
        assert!(read_request(&mut socket, &mut keys)
            .0
            .starts_with("RECORD "));
        reply(
            &mut socket,
            &mut keys,
            3,
            Some(profile::dict([("status", profile::uint(1))])),
        );
        assert!(read_request(&mut socket, &mut keys)
            .0
            .starts_with("TEARDOWN "));
        reply(&mut socket, &mut keys, 4, None);
    });
    let (event, _event_peer) = udp_pair();
    let (general, _general_peer) = udp_pair();
    owner
        .negotiate_with(|wire, setup, id| {
            let clock =
                ptp::Grandmaster::new(id, setup.clock_id, Instant::now(), Duration::from_secs(30))?;
            Ok((
                None,
                ptp::Service::attach(clock, event, general, wire.ptp_lease()?)?,
            ))
        })
        .unwrap();
    assert!(owner.progress(Stage::Stream).is_err());
    assert_eq!(owner.state(), State::Failed);
    assert!(owner.prepared.is_none());
    assert!(owner.control.is_none());
    server.join().unwrap();
}

#[test]
fn inband_rejections_at_every_control_stage_discard_session_and_key() {
    for rejected_seq in 3..=8 {
        let (mut owner, _approval, server) = fixture(move |mut socket| {
            let mut keys = server_keys();
            read_request(&mut socket, &mut keys);
            reply(&mut socket, &mut keys, 1, Some(info()));
            read_request(&mut socket, &mut keys);
            reply(&mut socket, &mut keys, 2, Some(setup(123)));
            for seq in 3..=rejected_seq {
                let (head, body) = read_request(&mut socket, &mut keys);
                assert!(!head.starts_with("TEARDOWN "));
                let value = if seq == rejected_seq {
                    Some(profile::dict([("status", profile::uint(7))]))
                } else if seq == 6 {
                    let value = profile::parse(&body).unwrap();
                    let stream = value.as_dictionary().unwrap()["streams"]
                        .as_array()
                        .unwrap()[0]
                        .as_dictionary()
                        .unwrap();
                    Some(profile::dict([(
                        "streams",
                        Value::Array(vec![profile::dict([
                            ("type", profile::uint(103)),
                            ("dataPort", profile::uint(12345)),
                            ("streamConnectionID", stream["streamConnectionID"].clone()),
                        ])]),
                    )]))
                } else {
                    None
                };
                reply(&mut socket, &mut keys, seq, value);
            }
            assert!(read_request(&mut socket, &mut keys)
                .0
                .starts_with("TEARDOWN "));
            reply(&mut socket, &mut keys, rejected_seq + 1, None);
        });
        let (event, peer_event) = udp_pair();
        let (general, _general_peer) = udp_pair();
        owner
            .negotiate_with(|wire, setup, id| {
                let clock = ptp::Grandmaster::new(
                    id,
                    setup.clock_id,
                    Instant::now(),
                    Duration::from_secs(30),
                )?;
                Ok((
                    None,
                    ptp::Service::attach(clock, event, general, wire.ptp_lease()?)?,
                ))
            })
            .unwrap();
        peer_event
            .send(
                &ptp::packet::Packet::new(ptp::packet::Kind::DelayReq, 123, 1, vec![0; 10])
                    .encode()
                    .unwrap(),
            )
            .unwrap();
        owner.observe(Duration::from_millis(1)).unwrap();
        assert!(owner.progress(Stage::Anchor).is_err());
        assert_eq!(owner.state(), State::Failed);
        assert!(owner.prepared.is_none());
        assert!(owner.stream.is_none());
        assert!(owner.timing.is_none());
        assert!(!owner.anchor_accepted);
        server.join().unwrap();
    }
}

#[test]
fn pending_record_cancel_revocation_timing_loss_and_tcp_close_fail_closed() {
    for fault in 0..4 {
        let (entered, pending) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let (mut owner, approval, server) = fixture(move |mut socket| {
            let mut keys = server_keys();
            read_request(&mut socket, &mut keys);
            reply(&mut socket, &mut keys, 1, Some(info()));
            read_request(&mut socket, &mut keys);
            reply(&mut socket, &mut keys, 2, Some(setup(123)));
            assert!(read_request(&mut socket, &mut keys)
                .0
                .starts_with("RECORD "));
            entered.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(2)).unwrap();
            if fault == 2 {
                // Response can be valid while the timing service has failed.
                thread::sleep(Duration::from_millis(160));
                reply(&mut socket, &mut keys, 3, None);
                assert!(read_request(&mut socket, &mut keys)
                    .0
                    .starts_with("TEARDOWN "));
                reply(&mut socket, &mut keys, 4, None);
            }
            // Other faults close TCP, with no further control stage possible.
        });
        let (event, _event_peer) = udp_pair();
        let (general, _general_peer) = udp_pair();
        owner
            .negotiate_with(|wire, setup, id| {
                let timeout = if fault == 2 {
                    Duration::from_millis(100)
                } else {
                    Duration::from_secs(30)
                };
                let clock = ptp::Grandmaster::new(id, setup.clock_id, Instant::now(), timeout)?;
                Ok((
                    None,
                    ptp::Service::attach(clock, event, general, wire.ptp_lease()?)?,
                ))
            })
            .unwrap();
        let cancel = owner.cancel.clone();
        let config = approval.path().join("config.toml");
        let change = thread::spawn(move || {
            pending.recv_timeout(Duration::from_secs(2)).unwrap();
            if fault == 0 {
                cancel.store(true, Ordering::Relaxed);
            }
            if fault == 1 {
                std::fs::write(config, "receiver_allowlist=[]").unwrap();
            }
            release.send(()).unwrap();
        });
        assert!(owner.progress(Stage::Stream).is_err());
        assert_eq!(owner.state(), State::Failed);
        assert!(owner.control.is_none());
        assert!(owner.timing.is_none());
        assert!(owner.prepared.is_none());
        assert!(!owner.anchor_accepted);
        change.join().unwrap();
        server.join().unwrap();
    }
}

#[test]
fn transient_rejection_never_allocates_ptp_events_or_stream_resources() {
    let listener = TcpListener::bind("127.0.0.2:0").unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let server = thread::spawn(move || {
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            peer.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        assert!(request.starts_with(b"GET /info RTSP/1.0\r\n"));
        peer.write_all(b"RTSP/1.0 403 Forbidden\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n")
            .unwrap();
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    });
    let mut wire = Wire::mock_method(
        socket,
        Duration::from_secs(1),
        "device:112233445566",
        crate::receiver::compatibility::Method::Transient,
    );
    let _authorization = wire.ptp_test_authorization().unwrap();
    let mut owner = Session::new();
    assert!(owner.start(|| Ok(wire), None).is_err());
    assert_eq!(owner.state(), State::Failed);
    assert!(!owner.attempted_setup);
    assert!(owner.control.is_none());
    assert!(owner.timing.is_none());
    assert!(owner.events.is_none());
    assert!(owner.prepared.is_none());
    assert!(owner.stream.is_none());
    assert!(!owner.anchor_accepted);
    assert!(owner.request_playback().is_err());
    server.join().unwrap();
}
