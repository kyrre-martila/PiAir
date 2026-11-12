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
                if TOKENLESS.with(std::cell::Cell::get) {
                    assert!(!header.to_ascii_lowercase().contains("\r\nsession:"));
                }
                return (header, bytes[end + 4..].to_vec());
            }
        }
        assert!(bytes.len() < 16384);
    }
}
fn reply(socket: &mut TcpStream, keys: &mut ControlKeys, seq: u32, value: Option<Value>) {
    reply_session(
        socket,
        keys,
        seq,
        value,
        if seq == 1 {
            ""
        } else {
            "Session: test-session\r\n"
        },
    );
}
fn reply_session(
    socket: &mut TcpStream,
    keys: &mut ControlKeys,
    seq: u32,
    value: Option<Value>,
    session: &str,
) {
    let session = TOKENLESS.with(|mode| if mode.get() { "" } else { session });
    let body = value
        .map(|mut v| profile::binary(&mut v).unwrap())
        .unwrap_or_default();
    let mut response = format!("RTSP/1.0 200 OK\r\nCSeq: {seq}\r\nContent-Type: application/x-apple-binary-plist\r\n{session}Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    response.extend_from_slice(&body);
    for part in response.chunks(31) {
        socket.write_all(&keys.encrypt(part).unwrap()).unwrap();
    }
}
thread_local! {
    static TOKENLESS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
fn fixture(
    server: impl FnOnce(TcpStream) + Send + 'static,
) -> (Session, tempfile::TempDir, thread::JoinHandle<()>) {
    fixture_auth(
        server,
        super::super::compatibility::Method::PinDisplay,
        false,
    )
}
fn fixture_auth(
    server: impl FnOnce(TcpStream) + Send + 'static,
    method: super::super::compatibility::Method,
    verified: bool,
) -> (Session, tempfile::TempDir, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.2:0").unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let tokenless = TOKENLESS.with(std::cell::Cell::get);
    let handle = thread::spawn(move || {
        TOKENLESS.with(|mode| mode.set(tokenless));
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        server(socket);
    });
    let mut wire = Wire::mock_method(
        socket,
        Duration::from_millis(500),
        "device:112233445566",
        method,
    );
    let authorization = wire.ptp_test_authorization().unwrap();
    let mut wire = wire
        .secure(ControlKeys::new(b"lifecycle"), ControlKeys::new(b"events"))
        .unwrap();
    wire.install_audio_key(&[0x62; 32]).unwrap();
    if verified {
        wire.bind_stored_hap_verification(pairing::verified_stored_hap_for_test());
    }
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
fn state_machine_requires_timing_before_playing_and_rejects_stale_operations() {
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
            if from == State::Timing && to == State::Playing {
                state.advance(to).unwrap();
                assert_eq!(state, State::Playing);
            } else {
                assert!(state.advance(to).is_err());
                assert_eq!(state, from);
            }
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
    assert!(validate_info(&info(), "device:112233445566", None).is_ok());
    assert!(validate_info(&info(), "device:223344556677", None).is_err());
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
            validate_info(&value_info, "device:112233445566", None).is_err(),
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
                Value::String("bad numeric IP".into()),
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
        assert_eq!(
            head.contains("Session: test-session\r\n"),
            !TOKENLESS.with(std::cell::Cell::get)
        );
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
    for tokenless in [false, true] {
        TOKENLESS.with(|mode| mode.set(tokenless));
        partial_setup_faults_attempt_teardown_and_release_all_resources_case();
    }
    TOKENLESS.with(|mode| mode.set(false));
}
fn partial_setup_faults_attempt_teardown_and_release_all_resources_case() {
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
    for tokenless in [false, true] {
        TOKENLESS.with(|mode| mode.set(tokenless));
        cancellation_clock_loss_and_drop_attempt_authenticated_teardown_case();
    }
    TOKENLESS.with(|mode| mode.set(false));
}
fn cancellation_clock_loss_and_drop_attempt_authenticated_teardown_case() {
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
    for tokenless in [false, true] {
        TOKENLESS.with(|mode| mode.set(tokenless));
        revocation_prevents_timing_and_teardown_network_writes_case();
    }
    TOKENLESS.with(|mode| mode.set(false));
}
fn revocation_prevents_timing_and_teardown_network_writes_case() {
    let (mut owner, approval, server) = fixture(|socket| setup_server(socket, false));
    owner.negotiate_with(attach_test_timing).unwrap();
    std::fs::write(approval.path().join("config.toml"), "receiver_allowlist=[]").unwrap();
    let error = owner.observe(Duration::from_millis(1)).unwrap_err();
    assert!(format!("{error:#}").contains("revoked"));
    assert!(owner
        .diagnostic()
        .contains("TEARDOWN: unacknowledged; remote expiry unconfirmed"));
    assert!(owner.control.is_none());
    assert!(owner.timing.is_none());
    server.join().unwrap();
}
#[test]
fn teardown_stall_is_bounded_and_does_not_leave_a_control_socket() {
    for tokenless in [false, true] {
        TOKENLESS.with(|mode| mode.set(tokenless));
        teardown_stall_is_bounded_and_does_not_leave_a_control_socket_case();
    }
    TOKENLESS.with(|mode| mode.set(false));
}
fn teardown_stall_is_bounded_and_does_not_leave_a_control_socket_case() {
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
    assert!(owner
        .diagnostic()
        .contains("TEARDOWN: unacknowledged; remote expiry unconfirmed"));
    assert_eq!(owner.state(), State::Failed);
    assert!(owner.control.is_none());
    assert!(owner.timing.is_none());
    release.send(()).unwrap();
    server.join().unwrap();
}
#[test]
fn disconnect_during_setup_closes_without_reusing_cipher_or_retrying() {
    for tokenless in [false, true] {
        TOKENLESS.with(|mode| mode.set(tokenless));
        disconnect_during_setup_closes_without_reusing_cipher_or_retrying_case();
    }
    TOKENLESS.with(|mode| mode.set(false));
}
fn disconnect_during_setup_closes_without_reusing_cipher_or_retrying_case() {
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
    for tokenless in [false, true] {
        TOKENLESS.with(|mode| mode.set(tokenless));
        control_stages_validate_order_payloads_and_never_connect_media_case();
    }
    TOKENLESS.with(|mode| mode.set(false));
}
fn control_stages_validate_order_payloads_and_never_connect_media_case() {
    for target in [Stage::Record, Stage::Stream, Stage::Anchor] {
        let media = TcpListener::bind("127.0.0.2:0").unwrap();
        media.set_nonblocking(true).unwrap();
        let port = media.local_addr().unwrap().port();
        let (mut owner, _approval, server) = fixture(move |mut socket| {
            let mut keys = server_keys();
            let mut binding = None;
            read_bound_request(&mut socket, &mut keys, &mut binding);
            reply(&mut socket, &mut keys, 1, Some(info()));
            read_bound_request(&mut socket, &mut keys, &mut binding);
            reply(&mut socket, &mut keys, 2, Some(setup(123)));
            let (head, body) = read_bound_request(&mut socket, &mut keys, &mut binding);
            assert!(head.starts_with("RECORD "));
            assert!(body.is_empty());
            // PTP must continue while this response is pending.
            thread::sleep(Duration::from_millis(160));
            reply(&mut socket, &mut keys, 3, None);
            let mut next = 4;
            if target >= Stage::Stream {
                let (head, body) = read_bound_request(&mut socket, &mut keys, &mut binding);
                assert!(head.starts_with("SETPEERS "));
                assert!(head.contains("Content-Type: /peer-list-changed\r\n"));
                assert_eq!(
                    profile::parse(&body).unwrap(),
                    Value::Array(vec!["127.0.0.2".into(), "127.0.0.1".into()])
                );
                reply(&mut socket, &mut keys, 4, None);
                let (head, body) = read_bound_request(&mut socket, &mut keys, &mut binding);
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
                let (head, body) = read_bound_request(&mut socket, &mut keys, &mut binding);
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
                let (head, body) = read_bound_request(&mut socket, &mut keys, &mut binding);
                assert!(head.starts_with("POST /audioMode "));
                assert_eq!(
                    profile::parse(&body).unwrap(),
                    profile::dict([("audioMode", "default".into())])
                );
                reply(&mut socket, &mut keys, 7, None);
                next = 8;
            }
            if target == Stage::Anchor {
                let (head, body) = read_bound_request(&mut socket, &mut keys, &mut binding);
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
            let (head, _) = read_bound_request(&mut socket, &mut keys, &mut binding);
            assert!(head.starts_with("TEARDOWN "));
            assert_eq!(
                head.contains("Session: test-session\r\n"),
                !TOKENLESS.with(std::cell::Cell::get)
            );
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
        assert!(owner.progress_inner(target).is_err());
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
        let accepted = match target {
            Stage::Record => "RECORD",
            Stage::Stream => "audioMode",
            Stage::Anchor => "paused anchor",
        };
        assert!(owner
            .diagnostic()
            .contains(&format!("last accepted: {accepted};")));
        assert!(owner.diagnostic().contains("TEARDOWN: acknowledged"));
        assert!(owner
            .diagnostic()
            .contains("receiver clock adoption: unverified"));
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
    let error = owner.progress(Stage::Stream).unwrap_err();
    assert!(error.to_string().contains("rejected in-band: status=1"));
    assert!(owner
        .diagnostic()
        .contains("last accepted: initial SETUP; attempted: RECORD"));
    assert!(owner.diagnostic().contains("TEARDOWN: acknowledged"));
    assert!(owner
        .diagnostic()
        .contains("receiver clock adoption: unverified"));
    assert_eq!(owner.state(), State::Failed);
    assert!(owner.prepared.is_none());
    assert!(owner.control.is_none());
    server.join().unwrap();
}

#[test]
fn inband_rejections_at_every_control_stage_discard_session_and_key() {
    for tokenless in [false, true] {
        TOKENLESS.with(|mode| mode.set(tokenless));
        inband_rejections_at_every_control_stage_discard_session_and_key_case();
    }
    TOKENLESS.with(|mode| mode.set(false));
}
fn inband_rejections_at_every_control_stage_discard_session_and_key_case() {
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
        let (accepted, attempted) = match rejected_seq {
            3 => ("initial SETUP", "RECORD"),
            4 => ("RECORD", "SETPEERS"),
            5 => ("SETPEERS", "command registration"),
            6 => ("command registration", "stream SETUP"),
            7 => ("stream SETUP", "audioMode"),
            8 => ("audioMode", "paused anchor"),
            _ => unreachable!(),
        };
        assert!(owner.diagnostic().contains(&format!(
            "last accepted: {accepted}; attempted: {attempted}"
        )));
        assert!(owner.diagnostic().contains("PTP snapshot: Some(Shutdown)"));
        assert!(owner.diagnostic().contains("TEARDOWN: acknowledged"));
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
    for tokenless in [false, true] {
        TOKENLESS.with(|mode| mode.set(tokenless));
        pending_record_cancel_revocation_timing_loss_and_tcp_close_fail_closed_case();
    }
    TOKENLESS.with(|mode| mode.set(false));
}
fn pending_record_cancel_revocation_timing_loss_and_tcp_close_fail_closed_case() {
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
        assert!(owner
            .diagnostic()
            .contains("last accepted: initial SETUP; attempted: RECORD"));
        assert!(owner.diagnostic().contains(if fault == 2 {
            "TEARDOWN: acknowledged"
        } else {
            "TEARDOWN: unacknowledged; remote expiry unconfirmed"
        }));
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

#[test]
fn rejected_formats_send_no_setup_and_allocate_no_timing() {
    for formats in [
        None,
        Some(Value::Array(vec![])),
        Some(Value::Array(vec![profile::uint(23)])),
        Some(Value::String("PRIVATE".into())),
    ] {
        let (mut owner, _approval, server) = fixture(move |mut socket| {
            let mut keys = server_keys();
            let (head, _) = read_request(&mut socket, &mut keys);
            assert!(head.starts_with("GET /info "));
            let mut rejected = info();
            let root = rejected.as_dictionary_mut().unwrap();
            root.remove("supportedFormats");
            if let Some(formats) = formats {
                root.insert(
                    "supportedAudioFormatsExtended".into(),
                    profile::dict([("bufferStream", formats)]),
                );
            }
            reply(&mut socket, &mut keys, 1, Some(rejected));
            let mut byte = [0];
            assert_eq!(socket.read(&mut byte).unwrap(), 0);
        });
        let error = owner
            .negotiate_with(|_, _, _| panic!("PTP allocation after rejection"))
            .unwrap_err();
        assert!(error.to_string().contains("required21=false"));
        assert!(!owner.attempted_setup);
        assert!(owner.timing.is_none());
        assert!(owner.events.is_none());
        drop(owner);
        server.join().unwrap();
    }
}

#[test]
fn signed_mask_reconciliation_reaches_the_existing_authenticated_gate() {
    for (mask, list, accepted) in [
        (i64::MIN + 2097152, vec![21, 23, 72], true),
        (i64::MIN + 2097152, vec![21, 63], true),
        (i64::MIN, vec![21, 72], false),
        (i64::MIN + 2097152, vec![23, 72], false),
        (i64::MIN, vec![23, 72], false),
        (i64::MIN + 2097152, vec![21, 21], false),
    ] {
        let mut value = info();
        let root = value.as_dictionary_mut().unwrap();
        root.insert(
            "supportedFormats".into(),
            profile::dict([("bufferStream", Value::Integer(mask.into()))]),
        );
        root.insert(
            "supportedAudioFormatsExtended".into(),
            profile::dict([(
                "bufferStream",
                Value::Array(list.into_iter().map(profile::uint).collect()),
            )]),
        );
        assert_eq!(
            validate_info(&value, "device:112233445566", None).is_ok(),
            accepted
        );
    }
}

#[test]
fn advertised_timing_selection_is_bounded_and_deterministic() {
    fn select(authorized: &str, addresses: &[&str]) -> Result<IpAddr> {
        let values: Vec<_> = addresses
            .iter()
            .map(|s| Value::String((*s).into()))
            .collect();
        TimingPeer::select(&values, authorized.parse().unwrap()).map(TimingPeer::ip)
    }
    let authorized = "192.168.1.119";
    for addresses in [
        vec![authorized],
        vec!["192.168.1.120", authorized],
        vec![authorized, authorized],
    ] {
        assert_eq!(
            select(authorized, &addresses).unwrap().to_string(),
            authorized
        );
    }
    assert_eq!(
        select(authorized, &["192.168.1.120", "192.168.1.121"])
            .unwrap()
            .to_string(),
        "192.168.1.120"
    );
    assert_eq!(
        select(authorized, &["2001:db8::1", "192.168.1.120"])
            .unwrap()
            .to_string(),
        "192.168.1.120"
    );
    assert_eq!(
        select("2001:db8::1", &["192.168.1.119", "2001:db8::2"])
            .unwrap()
            .to_string(),
        "2001:db8::2"
    );
    for addresses in [
        vec![],
        vec!["not-an-ip"],
        vec!["192.168.1.119", "bad"],
        vec!["0.0.0.0", "224.0.0.1", "255.255.255.255", "127.0.0.1"],
        vec!["::ffff:192.168.1.119"],
        vec![authorized; 33],
    ] {
        assert!(select(authorized, &addresses).is_err(), "{addresses:?}");
    }
    for address in [
        "fe80::1",
        "fe80::1%eth0",
        "::",
        "ff02::1",
        "::1",
        "::ffff:192.168.1.119",
    ] {
        assert!(select("2001:db8::1", &[address]).is_err(), "{address}");
    }
    assert!(TimingPeer::select(&[Value::Integer(1.into())], authorized.parse().unwrap()).is_err());
}

#[test]
fn selected_advertised_peer_is_carried_with_authenticated_clock() {
    let mut response = setup(123);
    response
        .as_dictionary_mut()
        .unwrap()
        .get_mut("timingPeerInfo")
        .unwrap()
        .as_dictionary_mut()
        .unwrap()
        .insert(
            "Addresses".into(),
            Value::Array(vec![Value::String("192.168.1.120".into())]),
        );
    let selected = InitialSetup::parse(&response, "192.168.1.119".parse().unwrap(), 42).unwrap();
    assert_eq!(selected.peer.ip().to_string(), "192.168.1.120");
    assert_eq!(selected.clock_id, 123);
}

#[test]
fn advertised_address_failure_and_selected_peer_attachment_failure_teardown() {
    for malformed in [true, false] {
        let (mut owner, _approval, server) = fixture(move |mut socket| {
            let mut keys = server_keys();
            read_request(&mut socket, &mut keys);
            reply(&mut socket, &mut keys, 1, Some(info()));
            read_request(&mut socket, &mut keys);
            let mut response = setup(123);
            response
                .as_dictionary_mut()
                .unwrap()
                .get_mut("timingPeerInfo")
                .unwrap()
                .as_dictionary_mut()
                .unwrap()
                .insert(
                    "Addresses".into(),
                    Value::Array(vec![Value::String(
                        if malformed {
                            "malformed"
                        } else {
                            "192.168.1.120"
                        }
                        .into(),
                    )]),
                );
            reply(&mut socket, &mut keys, 2, Some(response));
            let (head, _) = read_request(&mut socket, &mut keys);
            assert!(head.starts_with("TEARDOWN "));
            reply(&mut socket, &mut keys, 3, None);
        });
        let result = owner.negotiate_with(|_, setup, _| {
            assert!(!malformed);
            assert_eq!(setup.peer.ip().to_string(), "192.168.1.120");
            assert_eq!(setup.clock_id, 123);
            bail!("timing interface unavailable (injected)")
        });
        let error = owner.finish(result).unwrap_err();
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains(if malformed {
            "address validation failed"
        } else {
            "interface unavailable"
        }));
        assert_eq!(owner.state(), State::Failed);
        assert!(owner.control.is_none() && owner.timing.is_none() && owner.events.is_none());
        server.join().unwrap();
    }
}

// Retain UDP peers until cleanup; dropping connected peers can deliver ICMP
// connection-refused errors during the scoped PTP worker's next poll.
fn negotiate_retaining_timing_peers(owner: &mut Session) -> (UdpSocket, UdpSocket) {
    let (event, peer_event) = udp_pair();
    let (general, peer_general) = udp_pair();
    owner
        .negotiate_with(|wire, setup, id| {
            Ok((
                None,
                ptp::Service::attach(
                    ptp::Grandmaster::new(
                        id,
                        setup.clock_id,
                        Instant::now(),
                        Duration::from_secs(30),
                    )?,
                    event,
                    general,
                    wire.ptp_lease()?,
                )?,
            ))
        })
        .unwrap();
    (peer_event, peer_general)
}

// Check every request in the full staged exchange, including endpoint POSTs.
fn read_bound_request(
    socket: &mut TcpStream,
    keys: &mut ControlKeys,
    binding: &mut Option<String>,
) -> (String, Vec<u8>) {
    let (head, body) = read_request(socket, keys);
    let uri = head.split_whitespace().nth(1).unwrap();
    assert_eq!(
        head.contains("Session: test-session\r\n"),
        binding.is_some() && !TOKENLESS.with(std::cell::Cell::get)
    );
    if uri.starts_with("rtsp://") {
        if let Some(initial) = binding {
            assert_eq!(uri, initial);
        } else {
            let value = profile::parse(&body).unwrap();
            let uuid = value.as_dictionary().unwrap()["sessionUUID"]
                .as_string()
                .unwrap();
            assert!(uri.ends_with(&uuid.to_ascii_lowercase()));
            *binding = Some(uri.to_owned());
        }
    }
    (head, body)
}

fn raw_reply(keys: &mut ControlKeys, seq: u32, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "RTSP/1.0 200 OK\r\nCSeq: {seq}\r\n{headers}Content-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    keys.encrypt(&bytes).unwrap().to_vec()
}

#[test]
fn session_binding_rejects_changed_late_malformed_conflicting_and_replayed_controls() {
    for supplied in [false, true] {
        for fault in 0..8 {
            let (mut owner, _approval, server) = fixture(move |mut socket| {
                let mut keys = server_keys();
                read_request(&mut socket, &mut keys);
                reply(&mut socket, &mut keys, 1, Some(info()));
                let (head, body) = read_request(&mut socket, &mut keys);
                assert!(!head.contains("\r\nSession:"));
                let uri = head.split_whitespace().nth(1).unwrap().to_owned();
                let value = profile::parse(&body).unwrap();
                let uuid = value.as_dictionary().unwrap()["sessionUUID"]
                    .as_string()
                    .unwrap();
                assert!(uri.ends_with(&uuid.to_ascii_lowercase()));
                let token = if supplied {
                    "Session: test-session\r\n"
                } else {
                    ""
                };
                let setup_body = profile::binary(&mut setup(123)).unwrap();
                let setup_headers =
                    format!("Content-Type: application/x-apple-binary-plist\r\n{token}");
                let setup_frame = raw_reply(&mut keys, 2, &setup_headers, &setup_body);
                socket.write_all(&setup_frame).unwrap();
                let (head, _) = read_request(&mut socket, &mut keys);
                assert!(head.starts_with(&format!("RECORD {uri} RTSP/1.0\r\n")));
                assert_eq!(head.contains("Session: test-session\r\n"), supplied);
                let headers = match fault {
                    0 => "Session: other-session\r\n",
                    1 => "Session: \r\n",
                    2 => "Session: test-session;timeout=30\r\n",
                    3 => "Session: bad/token\r\n",
                    4 => "Session: test-session\r\nsEsSiOn: other-session\r\n",
                    _ => token,
                };
                let frame = if fault == 6 {
                    // Exact authenticated SETUP frame replayed at a new cipher counter.
                    setup_frame
                } else {
                    let mut frame =
                        raw_reply(&mut keys, if fault == 5 { 2 } else { 3 }, headers, &[]);
                    if fault == 7 {
                        *frame.last_mut().unwrap() ^= 1;
                    }
                    frame
                };
                socket.write_all(&frame).unwrap();
                if fault < 4 {
                    // Semantic token failure: bounded cleanup retains the original binding.
                    let (head, _) = read_request(&mut socket, &mut keys);
                    assert!(head.starts_with(&format!("TEARDOWN {uri} RTSP/1.0\r\n")));
                    assert_eq!(head.contains("Session: test-session\r\n"), supplied);
                    let frame = raw_reply(&mut keys, 4, token, &[]);
                    socket.write_all(&frame).unwrap();
                }
                assert_eq!(socket.read(&mut [0]).unwrap(), 0);
            });
            let _timing_peers = negotiate_retaining_timing_peers(&mut owner);
            let error = owner.progress(Stage::Stream).unwrap_err();
            let expected = match fault {
                0 => "receiver session changed or unexpectedly introduced",
                1..=3 => "unsupported receiver session token",
                4 => "duplicate receiver header",
                5 => "unexpected receiver CSeq",
                _ => "authentication tag invalid",
            };
            assert!(
                format!("{error:#}").contains(expected),
                "supplied={supplied} fault={fault}: {error:#}"
            );
            assert!(owner.diagnostic().contains(if fault < 4 {
                "TEARDOWN: acknowledged"
            } else {
                "TEARDOWN: unacknowledged; remote expiry unconfirmed"
            }));
            assert_eq!(owner.state(), State::Failed);
            assert_eq!(owner.stage, None);
            assert!(owner.control.is_none() && owner.timing.is_none());
            assert!(owner.prepared.is_none() && owner.stream.is_none());
            assert!(owner
                .diagnostic()
                .contains("last accepted: initial SETUP; attempted: RECORD"));
            server.join().unwrap();
        }
    }
}

#[test]
fn session_token_can_be_omitted_after_binding_but_is_still_reused_on_requests() {
    let (mut owner, _approval, server) = fixture(|mut socket| {
        let mut keys = server_keys();
        read_request(&mut socket, &mut keys);
        reply(&mut socket, &mut keys, 1, Some(info()));
        read_request(&mut socket, &mut keys);
        reply(&mut socket, &mut keys, 2, Some(setup(123)));
        for seq in 3..=4 {
            let (head, _) = read_request(&mut socket, &mut keys);
            assert!(head.starts_with(if seq == 3 { "RECORD " } else { "TEARDOWN " }));
            assert!(head.contains("Session: test-session\r\n"));
            reply_session(&mut socket, &mut keys, seq, None, "");
        }
    });
    let _timing_peers = negotiate_retaining_timing_peers(&mut owner);
    owner.progress(Stage::Record).unwrap();
    owner.stop().unwrap();
    server.join().unwrap();
}

#[test]
fn initial_session_headers_are_strict_and_info_cannot_establish_a_token() {
    for (early, header, transport_failure) in [
        (true, "Session: early\r\n".to_owned(), false),
        (false, "Session: \r\n".into(), false),
        (false, "Session: token;timeout=30\r\n".into(), false),
        (false, format!("Session: {}\r\n", "a".repeat(129)), false),
        (false, "Session: one\r\nsession: two\r\n".into(), true),
        (false, "Session: bad\u{7f}\r\n".into(), true),
    ] {
        let (mut owner, _approval, server) = fixture(move |mut socket| {
            let mut keys = server_keys();
            read_request(&mut socket, &mut keys);
            if !early {
                reply(&mut socket, &mut keys, 1, Some(info()));
                read_request(&mut socket, &mut keys);
            }
            let body = profile::binary(&mut if early { info() } else { setup(123) }).unwrap();
            let frame = raw_reply(
                &mut keys,
                if early { 1 } else { 2 },
                &format!("Content-Type: application/x-apple-binary-plist\r\n{header}"),
                &body,
            );
            socket.write_all(&frame).unwrap();
            if !early && !transport_failure {
                let (head, _) = read_request(&mut socket, &mut keys);
                assert!(head.starts_with("TEARDOWN "));
                assert!(!head.contains("\r\nSession:"));
                reply_session(&mut socket, &mut keys, 3, None, "");
            }
            assert_eq!(socket.read(&mut [0]).unwrap(), 0);
        });
        let result =
            owner.negotiate_with(|_, _, _| panic!("invalid SETUP must not allocate timing"));
        assert!(owner.finish(result).is_err());
        assert_eq!(owner.state(), State::Failed);
        assert!(!owner.session_bound);
        assert!(owner.control.is_none());
        server.join().unwrap();
    }
}

#[test]
fn teardown_cannot_introduce_or_change_the_session_binding() {
    for supplied in [false, true] {
        let (mut owner, _approval, server) = fixture(move |mut socket| {
            let mut keys = server_keys();
            read_request(&mut socket, &mut keys);
            reply(&mut socket, &mut keys, 1, Some(info()));
            read_request(&mut socket, &mut keys);
            reply_session(
                &mut socket,
                &mut keys,
                2,
                Some(setup(123)),
                if supplied {
                    "Session: test-session\r\n"
                } else {
                    ""
                },
            );
            let (head, _) = read_request(&mut socket, &mut keys);
            assert!(head.starts_with("TEARDOWN "));
            assert_eq!(head.contains("Session: test-session\r\n"), supplied);
            reply_session(&mut socket, &mut keys, 3, None, "Session: unexpected\r\n");
            assert_eq!(socket.read(&mut [0]).unwrap(), 0);
        });
        let _timing_peers = negotiate_retaining_timing_peers(&mut owner);
        assert!(owner.stop().is_err());
        assert_eq!(owner.state(), State::Failed);
        assert!(!owner.session_bound);
        assert!(owner.token.is_none() && owner.control.is_none() && owner.timing.is_none());
        assert!(owner
            .diagnostic()
            .contains("TEARDOWN: unacknowledged; remote expiry unconfirmed"));
        server.join().unwrap();
    }
}

#[test]
fn authenticated_handoff_retains_cipher_cseq_cancel_and_receiver_without_setup() {
    let (mut original, _approval, server) = fixture(|mut socket| {
        let mut keys = server_keys();
        for seq in 1..=2 {
            let (header, body) = read_request(&mut socket, &mut keys);
            assert!(header.starts_with("GET "));
            assert!(header.contains(&format!("CSeq: {seq}\r\n")));
            assert!(body.is_empty());
            socket
                .write_all(&raw_reply(&mut keys, seq, "", &[]))
                .unwrap();
        }
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    });
    original.call("GET", Some("/info"), &[]).unwrap();
    let wire = original.control.take().unwrap();
    let cancel = original.cancel.clone();
    let mut transferred = Session::from_authenticated(wire, cancel.clone()).unwrap();
    assert!(Arc::ptr_eq(&cancel, &transferred.cancel));
    assert_eq!(transferred.state(), State::Negotiation);
    assert!(!transferred.attempted_setup);
    assert!(transferred.timing.is_none());
    transferred.call("GET", Some("/info"), &[]).unwrap();
    transferred.stop().unwrap();
    server.join().unwrap();
}

#[test]
fn timing_only_handoff_orders_record_setpeers_and_releases_sockets() {
    for tokenless in [false, true] {
        TOKENLESS.with(|mode| mode.set(tokenless));
        let (mut original, _approval, server) = fixture(|mut socket| {
            let mut keys = server_keys();
            for (seq, method, response) in [
                (1, "GET /info ", Some(info())),
                (2, "SETUP ", Some(setup(123))),
                (3, "RECORD ", None),
                (4, "SETPEERS ", None),
                (5, "TEARDOWN ", None),
            ] {
                let (header, body) = read_request(&mut socket, &mut keys);
                assert!(header.starts_with(method));
                assert!(header.contains(&format!("CSeq: {seq}\r\n")));
                if seq == 2 {
                    let offer = profile::parse(&body).unwrap();
                    let d = profile::dictionary(&offer).unwrap();
                    assert_eq!(d["timingProtocol"].as_string(), Some("PTP"));
                    assert_eq!(
                        d["timingPeerList"].as_array().unwrap(),
                        &[d["timingPeerInfo"].clone()]
                    );
                    assert!(!d.contains_key("streams"));
                }
                reply(&mut socket, &mut keys, seq, response);
            }
        });
        let cancel = original.cancel.clone();
        let mut owner =
            Session::from_authenticated(original.control.take().unwrap(), cancel).unwrap();
        let id = owner.clock_id;
        let (event, peer_event) = udp_pair();
        let (general, _peer_general) = udp_pair();
        let addresses = (event.local_addr().unwrap(), general.local_addr().unwrap());
        owner
            .negotiate_with(|wire, setup, clock| {
                assert_eq!(clock, id);
                Ok((
                    None,
                    ptp::Service::attach(
                        ptp::Grandmaster::new(
                            clock,
                            setup.clock_id,
                            Instant::now(),
                            Duration::from_secs(30),
                        )?,
                        event,
                        general,
                        wire.ptp_lease()?,
                    )?,
                ))
            })
            .unwrap();
        owner.negotiate_timing_peers().unwrap();
        assert!(owner.prepared.is_none() && owner.stream.is_none());
        peer_event
            .send(
                &ptp::packet::Packet::new(ptp::packet::Kind::DelayReq, 123, 1, vec![0; 10])
                    .encode()
                    .unwrap(),
            )
            .unwrap();
        owner.observe(Duration::from_millis(2)).unwrap();
        let evidence = owner.timing.as_ref().unwrap().evidence();
        assert!(
            evidence.sync_sent > 0 && evidence.follow_up_sent > 0 && evidence.announce_sent > 0
        );
        assert_eq!(evidence.delay_requests, 1);
        assert_eq!(evidence.delay_responses_sent, 1);
        // Completed observation remains historical evidence even if the timing
        // service shuts down before the authorized TEARDOWN runs.
        assert_eq!(owner.observed_peer_liveness, Some(true));
        if tokenless {
            owner.timing.as_mut().unwrap().shutdown();
            assert_eq!(owner.timing.as_ref().unwrap().state(), ptp::State::Shutdown);
        }
        owner.stop().unwrap();
        assert!(owner.diagnostic().contains("PTP snapshot: Some(Shutdown)"));
        assert!(owner.diagnostic().contains("TEARDOWN: acknowledged"));
        assert!(owner.diagnostic().contains("audio bytes: 0"));
        assert!(owner
            .diagnostic()
            .contains("negotiation observation: completed"));
        assert!(owner.diagnostic().contains("synchronization: unverified"));
        assert!(owner
            .diagnostic()
            .contains("peer liveness at snapshot: true"));
        assert!(owner.diagnostic().contains("local cleanup: released"));
        let _event = UdpSocket::bind(addresses.0).unwrap();
        let _general = UdpSocket::bind(addresses.1).unwrap();
        server.join().unwrap();
        TOKENLESS.with(|mode| mode.set(false));
    }
}

#[test]
fn initial_ptp_offer_preserves_one_signed_clock_identity_and_verified_address() {
    let uuid = uuid::Uuid::nil();
    let local = "192.0.2.1".parse().unwrap();
    for clock in [1, 0x8123456789abcdef, u64::MAX - 1] {
        let bytes = initial_offer("02:01:02:03:04:05", local, clock, uuid).unwrap();
        let value = Value::from_reader(std::io::Cursor::new(&*bytes)).unwrap();
        let root = profile::dictionary(&value).unwrap();
        let peer = profile::dictionary(&root["timingPeerInfo"]).unwrap();
        assert_eq!(peer["ClockID"].as_signed_integer().unwrap() as u64, clock);
        assert_eq!(
            peer["Addresses"].as_array().unwrap(),
            &[Value::String(local.to_string())]
        );
        assert_eq!(
            peer["SupportsClockPortMatchingOverride"].as_boolean(),
            Some(false)
        );
        assert_eq!(
            root["timingPeerList"].as_array().unwrap(),
            &[root["timingPeerInfo"].clone()]
        );
        assert!(!root.contains_key("timingPort") && !root.contains_key("streams"));
        let now = Instant::now();
        let mut gm = ptp::Grandmaster::new(clock, 123, now, Duration::from_secs(30)).unwrap();
        for packet in gm.due(now).unwrap() {
            assert_eq!(&packet.source[..8], &clock.to_be_bytes());
        }
    }
    for clock in [0, u64::MAX] {
        assert!(initial_offer("sender", local, clock, uuid).is_err());
    }
    assert!(initial_offer("sender", "0.0.0.0".parse().unwrap(), 1, uuid).is_err());
}

#[test]
fn authenticated_policy_reports_exact_fields_without_guessing_from_discovery() {
    // Policy combinations are synthetic; 99908 is the confirmed Stua statusFlags.
    for flags in [0, 4, 99908, 0x98404, 1 << 3, 1 << 7, 1 << 9, 0x99908] {
        let mut value = info();
        value
            .as_dictionary_mut()
            .unwrap()
            .insert("statusFlags".into(), profile::uint(flags));
        let result = validate_info(&value, "device:112233445566", None);
        let rejected = flags & ((1 << 3) | (1 << 7) | (1 << 9));
        if rejected == 0 {
            result.unwrap();
        } else {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("rejected field=statusFlags"), "{error}");
            assert!(
                error.contains(&format!("statusFlags={flags} (0x{flags:x})")),
                "{error}"
            );
            assert!(
                error.contains(&format!("rejected mask=0x{rejected:x}")),
                "{error}"
            );
        }
    }
    for field in ["acl", "act", "accessControlLevel", "accessControlType"] {
        for v in [
            profile::uint(0),
            profile::uint(1),
            profile::uint(2),
            Value::Integer((-1).into()),
            Value::Boolean(false),
            Value::String("PRIVATE".into()),
        ] {
            let mut value = info();
            value
                .as_dictionary_mut()
                .unwrap()
                .insert(field.into(), v.clone());
            let result = validate_info(&value, "device:112233445566", None);
            if v.as_unsigned_integer() == Some(0) {
                result.unwrap();
            } else {
                let error = format!("{:#}", result.unwrap_err());
                assert!(
                    error.contains(&format!("rejected field={field}")),
                    "{error}"
                );
                assert!(!error.contains("PRIVATE"));
            }
        }
    }
    for field in ["isGroupLeader", "isGroupMember", "isProxyGroupPlayer"] {
        for v in [
            Value::Boolean(false),
            Value::Boolean(true),
            profile::uint(0),
        ] {
            let mut value = info();
            value
                .as_dictionary_mut()
                .unwrap()
                .insert(field.into(), v.clone());
            let result = validate_info(&value, "device:112233445566", None);
            assert_eq!(result.is_ok(), v.as_boolean() == Some(false));
            if let Err(error) = result {
                assert!(error
                    .to_string()
                    .contains(&format!("rejected field={field}")));
            }
        }
    }
    for v in [
        None,
        Some(Value::String("PRIVATE".into())),
        Some(Value::Data(b"PRIVATE".to_vec())),
        Some(Value::Array(vec![Value::String("PRIVATE".into())])),
        Some(Value::Integer((-1).into())),
    ] {
        let mut value = info();
        let root = value.as_dictionary_mut().unwrap();
        root.remove("statusFlags");
        if let Some(v) = v {
            root.insert("statusFlags".into(), v);
        }
        let error = format!(
            "{:#}",
            validate_info(&value, "device:112233445566", None).unwrap_err()
        );
        assert!(error.contains("statusFlags="));
        assert!(!error.contains("PRIVATE"));
        assert!(!info_policy_diagnostic(value.as_dictionary().unwrap()).contains("PRIVATE"));
    }
}

#[test]
fn rejected_authenticated_policy_closes_control_before_setup_without_resources() {
    for verified in [false, true] {
        for (field, value) in [
            ("statusFlags", profile::uint(0x99908)),
            ("acl", profile::uint(2)),
            ("accessControlType", Value::String("PRIVATE".into())),
            ("isProxyGroupPlayer", Value::Boolean(true)),
        ] {
            let (mut owner, _approval, server) = fixture_auth(
                move |mut socket| {
                    let mut keys = server_keys();
                    let (head, _) = read_request(&mut socket, &mut keys);
                    assert!(head.starts_with("GET /info "));
                    let mut rejected = info();
                    rejected
                        .as_dictionary_mut()
                        .unwrap()
                        .insert(field.into(), value);
                    reply(&mut socket, &mut keys, 1, Some(rejected));
                    // No SETUP, RECORD, SETPEERS, audio or premature TEARDOWN.
                    assert_eq!(socket.read(&mut [0]).unwrap(), 0);
                },
                super::super::compatibility::Method::StoredHap,
                verified,
            );
            let error = owner.setup_authenticated().unwrap_err();
            assert!(format!("{error:#}").contains(&format!("rejected field={field}")));
            assert_eq!(owner.state(), State::Failed);
            assert!(!owner.attempted_setup);
            assert!(owner.control.is_none() && owner.timing.is_none() && owner.events.is_none());
            assert!(owner.prepared.is_none() && owner.stream.is_none());
            assert!(owner.diagnostic().contains("audio bytes: 0"));
            assert!(owner
                .diagnostic()
                .contains("negotiation observation: incomplete"));
            assert!(owner.diagnostic().contains("synchronization: unverified"));
            assert!(owner
                .diagnostic()
                .contains("peer liveness at snapshot: false"));
            assert!(owner.diagnostic().contains("local cleanup: released"));
            assert!(owner.request_playback().is_err());
            server.join().unwrap();
        }
    }
}

#[test]
fn stored_hap_policy_exception_is_narrow() {
    let proof = pairing::verified_stored_hap_for_test();
    for flags in [99908, 0x200, 0x200 | 8, 0x200 | 128] {
        let mut value = info();
        value
            .as_dictionary_mut()
            .unwrap()
            .insert("statusFlags".into(), profile::uint(flags));
        assert_eq!(
            validate_info(&value, "device:112233445566", Some(&proof)).is_ok(),
            flags & (8 | 128) == 0
        );
        assert!(validate_info(&value, "device:112233445566", None).is_err());
        assert!(validate_info(&value, "device:223344556677", Some(&proof)).is_err());
        for field in [
            "acl",
            "act",
            "accessControlLevel",
            "accessControlType",
            "statusFlags",
            "isGroupLeader",
            "isGroupMember",
            "isProxyGroupPlayer",
        ] {
            for invalid in [
                profile::uint(1),
                Value::String("invalid".into()),
                Value::Integer((-1).into()),
            ] {
                if field == "statusFlags" && invalid.as_unsigned_integer() == Some(1) {
                    continue;
                }
                let mut invalid_info = value.clone();
                invalid_info
                    .as_dictionary_mut()
                    .unwrap()
                    .insert(field.into(), invalid);
                assert!(validate_info(&invalid_info, "device:112233445566", Some(&proof)).is_err());
            }
        }
    }
}

#[test]
fn stua_policy_reaches_setup_only_on_verified_stored_hap_wire() {
    use super::super::compatibility::Method;
    for (method, verified) in [
        (Method::StoredHap, true),
        (Method::StoredHap, false),
        (Method::Transient, false),
    ] {
        let accepted = method == Method::StoredHap && verified;
        let (mut owner, _approval, server) = fixture_auth(
            move |mut socket| {
                let mut keys = server_keys();
                let (head, _) = read_request(&mut socket, &mut keys);
                assert!(head.starts_with("GET /info "));
                let mut value = info();
                value
                    .as_dictionary_mut()
                    .unwrap()
                    .insert("statusFlags".into(), profile::uint(99908));
                reply(&mut socket, &mut keys, 1, Some(value));
                if accepted {
                    let (head, _) = read_request(&mut socket, &mut keys);
                    assert!(head.starts_with("SETUP "));
                    // End the mock at initial SETUP; no synchronization is simulated.
                } else {
                    assert_eq!(socket.read(&mut [0]).unwrap(), 0);
                }
            },
            method,
            verified,
        );
        let result = owner.setup_authenticated();
        assert!(result.is_err());
        assert_eq!(owner.attempted == "initial SETUP", accepted, "{result:?}");
        assert_eq!(owner.state(), State::Failed);
        assert!(owner.control.is_none() && owner.events.is_none() && owner.timing.is_none());
        server.join().unwrap();
    }
}

fn realtime_info() -> Value {
    let mut value = info();
    let d = value.as_dictionary_mut().unwrap();
    d.insert(
        "features".into(),
        profile::uint((1 << 9) | (1 << 19) | (1u64 << 41)),
    );
    d.insert(
        "supportedFormats".into(),
        profile::dict([("audioStream", profile::uint(profile::FORMAT))]),
    );
    value
}

#[test]
fn complete_realtime_ptp_negotiation_owns_udp_key_and_bounded_cleanup() {
    for tokenless in [false, true] {
        TOKENLESS.with(|mode| mode.set(tokenless));
        let data = UdpSocket::bind("127.0.0.2:0").unwrap();
        let control = UdpSocket::bind("127.0.0.2:0").unwrap();
        let data_port = data.local_addr().unwrap().port();
        let control_port = control.local_addr().unwrap().port();
        let (mut owner, _approval, server) = fixture(move |mut socket| {
            let mut keys = server_keys();
            for (seq, method, response) in [
                (1, "GET /info ", Some(realtime_info())),
                (2, "SETUP ", Some(setup(123))),
                (3, "RECORD ", None),
                (4, "SETPEERS ", None),
                (
                    5,
                    "SETUP ",
                    Some(profile::dict([
                        ("timingProtocol", Value::String("PTP".into())),
                        (
                            "streams",
                            Value::Array(vec![profile::dict([
                                ("type", profile::uint(96)),
                                ("dataPort", profile::uint(u64::from(data_port))),
                                ("controlPort", profile::uint(u64::from(control_port))),
                            ])]),
                        ),
                    ])),
                ),
                (6, "TEARDOWN ", None),
            ] {
                let (header, body) = read_request(&mut socket, &mut keys);
                assert!(header.starts_with(method), "{header}");
                assert!(header.contains(&format!("CSeq: {seq}\r\n")));
                if seq == 4 {
                    assert!(header.contains("Content-Type: /peer-list-changed\r\n"));
                    assert_eq!(profile::parse(&body).unwrap().as_array().unwrap().len(), 2);
                }
                if seq == 5 {
                    let offer = profile::parse(&body).unwrap();
                    let stream = profile::dictionary(&offer).unwrap()["streams"]
                        .as_array()
                        .unwrap();
                    let stream = profile::dictionary(&stream[0]).unwrap();
                    for (field, value) in [
                        ("type", 96),
                        ("audioFormat", profile::FORMAT),
                        ("sr", 44100),
                        ("spf", 352),
                        ("ct", 2),
                    ] {
                        assert_eq!(profile::integer(stream, field).unwrap(), value);
                    }
                    assert_eq!(stream["shk"].as_data().unwrap(), &[0x62; 32]);
                    assert!(profile::port(stream, "controlPort").is_ok());
                    assert!(!stream.contains_key("dataPort"));
                }
                reply(&mut socket, &mut keys, seq, response);
            }
        });
        owner.realtime_profile = true;
        let (event, peer_event) = udp_pair();
        let (general, _peer_general) = udp_pair();
        let addresses = (event.local_addr().unwrap(), general.local_addr().unwrap());
        owner
            .negotiate_with(|wire, setup, clock| {
                Ok((
                    None,
                    ptp::Service::attach(
                        ptp::Grandmaster::new(
                            clock,
                            setup.clock_id,
                            Instant::now(),
                            Duration::from_secs(30),
                        )?,
                        event,
                        general,
                        wire.ptp_lease()?,
                    )?,
                ))
            })
            .unwrap();
        assert!(owner.realtime_ready().is_err());
        owner.negotiate_realtime().unwrap();
        assert!(owner.prepared.is_none() && owner.stream.is_none());
        let mut delay = ptp::packet::Packet::new(ptp::packet::Kind::DelayReq, 123, 1, vec![0; 10]);
        delay.source[8..].copy_from_slice(&32969u16.to_be_bytes());
        peer_event.send(&delay.encode().unwrap()).unwrap();
        owner.observe(Duration::from_millis(4)).unwrap();
        owner.realtime_ready().unwrap();
        assert_eq!(owner.realtime_ports, Some((data_port, control_port)));
        // No RTP, RTCP, volume or buffered anchor requests are transmitted.
        for socket in [&data, &control] {
            socket.set_nonblocking(true).unwrap();
            assert_eq!(
                socket.recv(&mut [0; 2048]).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
        let local_control = owner
            .realtime_transport
            .as_ref()
            .unwrap()
            .local_control()
            .unwrap();
        owner.stop().unwrap();
        assert!(owner.realtime_encoder.is_none() && owner.realtime_transport.is_none());
        assert!(owner.realtime_ready().is_err());
        let _rebound = UdpSocket::bind(("127.0.0.1", local_control)).unwrap();
        let _event = UdpSocket::bind(addresses.0).unwrap();
        let _general = UdpSocket::bind(addresses.1).unwrap();
        assert!(owner
            .diagnostic()
            .contains("operational readiness at snapshot: true"));
        assert!(owner.diagnostic().contains("TEARDOWN: acknowledged"));
        server.join().unwrap();
    }
    TOKENLESS.with(|mode| mode.set(false));
}

#[test]
fn realtime_stream_rejection_and_disconnect_destroy_prepared_resources() {
    for fault in 0..6 {
        let (mut owner, _approval, server) = fixture(move |mut socket| {
            let mut keys = server_keys();
            for (seq, method, response) in [
                (1, "GET /info ", Some(realtime_info())),
                (2, "SETUP ", Some(setup(123))),
                (3, "RECORD ", None),
                (4, "SETPEERS ", None),
            ] {
                assert!(read_request(&mut socket, &mut keys).0.starts_with(method));
                reply(&mut socket, &mut keys, seq, response);
            }
            let (header, body) = read_request(&mut socket, &mut keys);
            assert!(header.starts_with("SETUP "));
            if fault == 5 {
                let response = b"RTSP/1.0 461 Unsupported Transport\r\nCSeq: 5\r\nSession: test-session\r\nContent-Length: 0\r\n\r\n";
                socket.write_all(&keys.encrypt(response).unwrap()).unwrap();
                // A rejected RTSP exchange invalidates the control wire. Cleanup
                // must close it rather than write TEARDOWN on a failed channel.
                assert_eq!(socket.read(&mut [0]).unwrap(), 0);
                return;
            }
            if fault == 4 {
                return;
            } // actual TCP disconnect during stream SETUP
            let offered = profile::parse(&body).unwrap();
            let connection = offered.as_dictionary().unwrap()["streams"]
                .as_array()
                .unwrap()[0]
                .as_dictionary()
                .unwrap()["streamConnectionID"]
                .as_unsigned_integer()
                .unwrap();
            let mut stream = profile::dict([
                ("type", profile::uint(96)),
                ("dataPort", profile::uint(6000)),
                ("controlPort", profile::uint(6001)),
                ("streamConnectionID", profile::uint(connection)),
            ]);
            let (field, bad) = match fault {
                0 => ("type", profile::uint(103)),
                1 => ("dataPort", profile::uint(0)),
                2 => ("controlPort", profile::uint(6000)),
                _ => ("streamConnectionID", profile::uint(connection + 1)),
            };
            stream
                .as_dictionary_mut()
                .unwrap()
                .insert(field.into(), bad);
            reply(
                &mut socket,
                &mut keys,
                5,
                Some(profile::dict([("streams", Value::Array(vec![stream]))])),
            );
            assert!(read_request(&mut socket, &mut keys)
                .0
                .starts_with("TEARDOWN "));
            reply(&mut socket, &mut keys, 6, None);
        });
        owner.realtime_profile = true;
        let (event, _peer_event) = udp_pair();
        let (general, _peer_general) = udp_pair();
        let addresses = (event.local_addr().unwrap(), general.local_addr().unwrap());
        owner
            .negotiate_with(|wire, setup, clock| {
                Ok((
                    None,
                    ptp::Service::attach(
                        ptp::Grandmaster::new(
                            clock,
                            setup.clock_id,
                            Instant::now(),
                            Duration::from_secs(30),
                        )?,
                        event,
                        general,
                        wire.ptp_lease()?,
                    )?,
                ))
            })
            .unwrap();
        let before = Instant::now();
        assert!(owner.negotiate_realtime().is_err());
        assert!(before.elapsed() < Duration::from_secs(2));
        assert_eq!(owner.state(), State::Failed);
        assert!(owner.realtime_encoder.is_none() && owner.realtime_transport.is_none());
        assert!(owner.control.is_none() && owner.timing.is_none());
        let _event = UdpSocket::bind(addresses.0).unwrap();
        let _general = UdpSocket::bind(addresses.1).unwrap();
        server.join().unwrap();
    }
}

#[test]
fn realtime_info_needs_audio_alac_ptp_but_no_buffered_capability_or_format() {
    let mut info = realtime_info();
    validate_info_profile(&info, "device:112233445566", None, true).unwrap();
    info.as_dictionary_mut().unwrap().insert(
        "supportedFormats".into(),
        profile::dict([("bufferStream", profile::uint(1 << 21))]),
    );
    validate_info_profile(&info, "device:112233445566", None, true).unwrap();
    for format in [profile::uint(0), Value::String("ALAC".into())] {
        info.as_dictionary_mut().unwrap().insert(
            "supportedFormats".into(),
            profile::dict([("audioStream", format)]),
        );
        assert!(validate_info_profile(&info, "device:112233445566", None, true).is_err());
    }
    for missing in [9, 19, 41] {
        let mut info = realtime_info();
        info.as_dictionary_mut().unwrap().insert(
            "features".into(),
            profile::uint(((1 << 9) | (1 << 19) | (1u64 << 41)) & !(1 << missing)),
        );
        assert!(validate_info_profile(&info, "device:112233445566", None, true).is_err());
    }
}
#[test]
fn complete_ptp_wav_lifecycle_encrypts_paces_syncs_retransmits_and_releases() {
    for tokenless in [false, true] {
        TOKENLESS.with(|mode| mode.set(tokenless));
        let data = UdpSocket::bind("127.0.0.2:0").unwrap();
        let control = UdpSocket::bind("127.0.0.2:0").unwrap();
        let data_port = data.local_addr().unwrap().port();
        let control_port = control.local_addr().unwrap().port();
        let (mut owner, _approval, server) = fixture(move |mut socket| {
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut keys = server_keys();
            for (seq, method, response) in [
                (1, "GET /info ", Some(realtime_info())),
                (2, "SETUP ", Some(setup(123))),
                (3, "RECORD ", None),
                (4, "SETPEERS ", None),
                (
                    5,
                    "SETUP ",
                    Some(profile::dict([
                        ("timingProtocol", Value::String("PTP".into())),
                        (
                            "streams",
                            Value::Array(vec![profile::dict([
                                ("type", profile::uint(96)),
                                ("dataPort", profile::uint(u64::from(data_port))),
                                ("controlPort", profile::uint(u64::from(control_port))),
                            ])]),
                        ),
                    ])),
                ),
                (6, "SET_PARAMETER ", None),
                (if tokenless { 6 } else { 7 }, "POST /feedback ", None),
                (if tokenless { 7 } else { 8 }, "TEARDOWN ", None),
            ] {
                if method == "SET_PARAMETER " && tokenless {
                    continue;
                }
                let (header, body) = read_request(&mut socket, &mut keys);
                if method == "SET_PARAMETER " {
                    assert_eq!(body, b"volume: -20.000000\r\n");
                    assert!(header.contains("Content-Type: text/parameters\r\n"));
                }
                assert!(header.starts_with(method), "{header}");
                assert!(header.contains(&format!("CSeq: {seq}\r\n")));
                if seq == 4 {
                    assert!(header.contains("Content-Type: /peer-list-changed\r\n"));
                    assert_eq!(profile::parse(&body).unwrap().as_array().unwrap().len(), 2);
                }
                if seq == 5 {
                    let offer = profile::parse(&body).unwrap();
                    let stream = profile::dictionary(&offer).unwrap()["streams"]
                        .as_array()
                        .unwrap();
                    let stream = profile::dictionary(&stream[0]).unwrap();
                    for (field, value) in [
                        ("type", 96),
                        ("audioFormat", profile::FORMAT),
                        ("sr", 44100),
                        ("spf", 352),
                        ("ct", 2),
                    ] {
                        assert_eq!(profile::integer(stream, field).unwrap(), value);
                    }
                    assert_eq!(stream["shk"].as_data().unwrap(), &[0x62; 32]);
                    assert!(profile::port(stream, "controlPort").is_ok());
                    assert!(!stream.contains_key("dataPort"));
                }
                reply(&mut socket, &mut keys, seq, response);
            }
        });
        owner.realtime_profile = true;
        let (event, peer_event) = udp_pair();
        let (general, _peer_general) = udp_pair();
        let addresses = (event.local_addr().unwrap(), general.local_addr().unwrap());
        owner
            .negotiate_with(|wire, setup, clock| {
                Ok((
                    None,
                    ptp::Service::attach(
                        ptp::Grandmaster::new(
                            clock,
                            setup.clock_id,
                            Instant::now(),
                            Duration::from_secs(30),
                        )?,
                        event,
                        general,
                        wire.ptp_lease()?,
                    )?,
                ))
            })
            .unwrap();
        assert!(owner.realtime_ready().is_err());
        owner.negotiate_realtime().unwrap();
        assert!(owner.prepared.is_none() && owner.stream.is_none());
        let mut delay = ptp::packet::Packet::new(ptp::packet::Kind::DelayReq, 123, 1, vec![0; 10]);
        delay.source[8..].copy_from_slice(&32969u16.to_be_bytes());
        peer_event.send(&delay.encode().unwrap()).unwrap();
        owner.observe(Duration::from_millis(4)).unwrap();
        owner.realtime_ready().unwrap();
        assert_eq!(owner.realtime_ports, Some((data_port, control_port)));
        // No RTP, RTCP, volume or buffered anchor requests are transmitted.
        for socket in [&data, &control] {
            socket.set_nonblocking(true).unwrap();
            assert_eq!(
                socket.recv(&mut [0; 2048]).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
        let local_control = owner
            .realtime_transport
            .as_ref()
            .unwrap()
            .local_control()
            .unwrap();
        let frames = vec![[1200, -2400]; 353];
        let expected_frames = frames.clone();
        let clock_id = owner.clock_id;
        owner.realtime_encoder.as_mut().unwrap().sequence = u16::MAX;
        owner.realtime_encoder.as_mut().unwrap().timestamp = u32::MAX - 100;
        let origin = owner.realtime_encoder.as_ref().unwrap().timestamp;
        let receiver = thread::spawn(move || {
            data.set_nonblocking(false).unwrap();
            control.set_nonblocking(false).unwrap();
            data.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            control
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let started = Instant::now();
            let mut packets = Vec::new();
            let mut nonces = std::collections::HashSet::new();
            let total = (353 + realtime::PLAYBACK_LEAD_SAMPLES as usize).div_ceil(profile::SPF);
            let mut duplicate = false;
            while packets.len() < total || !duplicate {
                let mut b = [0; 2048];
                let n = data.recv(&mut b).unwrap();
                let packet = b[..n].to_vec();
                if packets.first() == Some(&packet) {
                    duplicate = true;
                    continue;
                }
                let index = packets.len();
                assert_eq!(&packet[..2], &[0x80, if index == 0 { 0xe0 } else { 0x60 }]);
                assert_eq!(
                    u16::from_be_bytes(packet[2..4].try_into().unwrap()),
                    u16::MAX.wrapping_add(index as u16)
                );
                assert_eq!(
                    u32::from_be_bytes(packet[4..8].try_into().unwrap()),
                    origin.wrapping_add((index * profile::SPF) as u32)
                );
                let nonce: [u8; 8] = packet[n - 8..].try_into().unwrap();
                assert!(nonces.insert(nonce));
                let plain = crate::receiver::crypto::open(
                    &[0x62; 32],
                    &crate::receiver::crypto::nonce(&nonce),
                    &packet[12..n - 8],
                    &packet[4..12],
                )
                .unwrap();
                let pcm = crate::audio::alac::tests::decode_at_rate(&plain, 44100);
                for (offset, sample) in pcm.iter().enumerate() {
                    assert_eq!(
                        *sample,
                        expected_frames
                            .get(index * profile::SPF + offset)
                            .copied()
                            .unwrap_or([0; 2])
                    );
                }
                packets.push(packet);
                if index == 0 {
                    control
                        .send_to(
                            &[0x80, 0xd5, 0, 1, 0xff, 0xff, 0, 1],
                            ("127.0.0.1", local_control),
                        )
                        .unwrap();
                }
            }
            assert!(started.elapsed() >= Duration::from_millis(1900));
            let mut previous = None;
            for index in 0..2 {
                let mut sync = [0; 64];
                assert_eq!(control.recv(&mut sync).unwrap(), 28);
                assert_eq!(
                    &sync[..4],
                    &[if index == 0 { 0x90 } else { 0x80 }, 0xd7, 0, 6]
                );
                let current = u32::from_be_bytes(sync[4..8].try_into().unwrap());
                let nanos = u64::from_be_bytes(sync[8..16].try_into().unwrap());
                let next = u32::from_be_bytes(sync[16..20].try_into().unwrap()).wrapping_add(11025);
                assert_eq!(current, next.wrapping_sub(77175));
                assert_eq!(
                    u64::from_be_bytes(sync[20..28].try_into().unwrap()),
                    clock_id
                );
                if let Some((pos, ns)) = previous {
                    let delta = next.wrapping_sub(pos);
                    assert_eq!(
                        nanos - ns,
                        realtime::sample_time(u64::from(delta)).as_nanos() as u64
                    );
                } else {
                    assert_eq!(next, origin);
                }
                previous = Some((next, nanos));
            }
            packets.len() as u64
        });
        owner
            .play_wav(&frames, if tokenless { None } else { Some(-20) })
            .unwrap();
        assert_eq!(owner.playback_stats.packets, receiver.join().unwrap());
        assert_eq!(owner.playback_stats.requests, 1);
        assert_eq!(owner.playback_stats.resends, 1);
        assert_eq!(owner.playback_stats.syncs, 2);
        owner.stop().unwrap();
        assert!(owner.realtime_encoder.is_none() && owner.realtime_transport.is_none());
        assert!(owner.realtime_ready().is_err());
        let _rebound = UdpSocket::bind(("127.0.0.1", local_control)).unwrap();
        let _event = UdpSocket::bind(addresses.0).unwrap();
        let _general = UdpSocket::bind(addresses.1).unwrap();
        assert!(owner
            .diagnostic()
            .contains("operational readiness at snapshot: true"));
        assert!(owner.diagnostic().contains("TEARDOWN: acknowledged"));
        server.join().unwrap();
    }
    TOKENLESS.with(|mode| mode.set(false));
}

#[test]
fn playback_cancellation_timing_loss_and_receiver_disconnect_join_and_cleanup() {
    for (mode, live) in [
        ("cancel", false),
        ("timing-loss", false),
        ("disconnect", false),
        ("malformed", false),
        ("cancel", true),
        ("device", true),
        ("disconnect", true),
        ("startup", true),
        ("startup-cancel", true),
        ("startup-disconnect", true),
        ("slow-start", true),
        ("teardown-timeout", true),
        ("duration", true),
    ] {
        let closed = Arc::new(AtomicBool::new(false));
        let server_closed = closed.clone();
        let failure = Arc::new(AtomicBool::new(false));
        let failed = failure.clone();
        let opened = Arc::new(AtomicBool::new(false));
        let did_open = opened.clone();
        let gate = Arc::new(AtomicBool::new(false));
        let ready = gate.clone();
        let data = UdpSocket::bind("127.0.0.2:0").unwrap();
        let control = UdpSocket::bind("127.0.0.2:0").unwrap();
        let data_port = data.local_addr().unwrap().port();
        let control_port = control.local_addr().unwrap().port();
        let (mut owner, _approval, server) = fixture(move |mut socket| {
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut keys = server_keys();
            for (seq, method, response) in [
                (1, "GET /info ", Some(realtime_info())),
                (2, "SETUP ", Some(setup(123))),
                (3, "RECORD ", None),
                (4, "SETPEERS ", None),
                (
                    5,
                    "SETUP ",
                    Some(profile::dict([
                        ("timingProtocol", Value::String("PTP".into())),
                        (
                            "streams",
                            Value::Array(vec![profile::dict([
                                ("type", profile::uint(96)),
                                ("dataPort", profile::uint(u64::from(data_port))),
                                ("controlPort", profile::uint(u64::from(control_port))),
                            ])]),
                        ),
                    ])),
                ),
            ] {
                let (header, body) = read_request(&mut socket, &mut keys);
                assert!(header.starts_with(method), "{header}");
                assert!(header.contains(&format!("CSeq: {seq}\r\n")));
                if seq == 4 {
                    assert!(header.contains("Content-Type: /peer-list-changed\r\n"));
                    assert_eq!(profile::parse(&body).unwrap().as_array().unwrap().len(), 2);
                }
                if seq == 5 {
                    let offer = profile::parse(&body).unwrap();
                    let stream = profile::dictionary(&offer).unwrap()["streams"]
                        .as_array()
                        .unwrap();
                    let stream = profile::dictionary(&stream[0]).unwrap();
                    for (field, value) in [
                        ("type", 96),
                        ("audioFormat", profile::FORMAT),
                        ("sr", 44100),
                        ("spf", 352),
                        ("ct", 2),
                    ] {
                        assert_eq!(profile::integer(stream, field).unwrap(), value);
                    }
                    assert_eq!(stream["shk"].as_data().unwrap(), &[0x62; 32]);
                    assert!(profile::port(stream, "controlPort").is_ok());
                    assert!(!stream.contains_key("dataPort"));
                }
                reply(&mut socket, &mut keys, seq, response);
            }
            let mut seq = 6;
            loop {
                let (header, _) = read_request(&mut socket, &mut keys);
                if header.starts_with("TEARDOWN ") {
                    if live && mode != "startup" {
                        assert!(
                            server_closed.load(Ordering::Acquire),
                            "capture must join before TEARDOWN"
                        );
                    }
                    if mode == "teardown-timeout" {
                        thread::sleep(Duration::from_millis(500));
                        break;
                    }
                    reply(&mut socket, &mut keys, seq, None);
                    break;
                }
                assert!(header.starts_with("POST /feedback "));
                if matches!(mode, "disconnect" | "startup-disconnect") {
                    return;
                }
                reply(&mut socket, &mut keys, seq, None);
                seq += 1;
            }
        });
        owner.realtime_profile = true;
        let (event, peer_event) = udp_pair();
        let (general, _peer_general) = udp_pair();
        let addresses = (event.local_addr().unwrap(), general.local_addr().unwrap());
        owner
            .negotiate_with(|wire, setup, clock| {
                Ok((
                    None,
                    ptp::Service::attach(
                        ptp::Grandmaster::new(
                            clock,
                            setup.clock_id,
                            Instant::now(),
                            Duration::from_secs(30),
                        )?,
                        event,
                        general,
                        wire.ptp_lease()?,
                    )?,
                ))
            })
            .unwrap();
        assert!(owner.realtime_ready().is_err());
        owner.negotiate_realtime().unwrap();
        assert!(owner.prepared.is_none() && owner.stream.is_none());
        let mut delay = ptp::packet::Packet::new(ptp::packet::Kind::DelayReq, 123, 1, vec![0; 10]);
        delay.source[8..].copy_from_slice(&32969u16.to_be_bytes());
        peer_event.send(&delay.encode().unwrap()).unwrap();
        owner.observe(Duration::from_millis(4)).unwrap();
        owner.realtime_ready().unwrap();
        assert_eq!(owner.realtime_ports, Some((data_port, control_port)));
        // No RTP, RTCP, volume or buffered anchor requests are transmitted.
        for socket in [&data, &control] {
            socket.set_nonblocking(true).unwrap();
            assert_eq!(
                socket.recv(&mut [0; 2048]).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
        let local_control = owner
            .realtime_transport
            .as_ref()
            .unwrap()
            .local_control()
            .unwrap();

        let cancel = owner.cancel.clone();
        let (finished, playback_done) = std::sync::mpsc::channel();
        let receiver = thread::spawn(move || {
            data.set_nonblocking(false).unwrap();
            data.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
            let mut packet = [0; 2048];
            if matches!(mode, "startup" | "startup-cancel" | "startup-disconnect") {
                if mode == "startup-cancel" {
                    let until = Instant::now() + Duration::from_secs(2);
                    while !did_open.load(Ordering::Acquire) {
                        assert!(Instant::now() < until);
                        thread::park_timeout(Duration::from_millis(1));
                    }
                    cancel.store(true, Ordering::Relaxed);
                }
                playback_done.recv_timeout(Duration::from_secs(5)).unwrap();
                data.set_nonblocking(true).unwrap();
                assert_eq!(
                    data.recv(&mut packet).unwrap_err().kind(),
                    std::io::ErrorKind::WouldBlock
                );
                return;
            }
            if mode == "slow-start" {
                let until = Instant::now() + Duration::from_secs(4);
                while Instant::now() < until {
                    delay.sequence = delay.sequence.wrapping_add(1);
                    peer_event.send(&delay.encode().unwrap()).unwrap();
                    thread::park_timeout(Duration::from_millis(100));
                }
                ready.store(true, Ordering::Release);
            }
            assert!(data.recv(&mut packet).unwrap() > 12);
            if matches!(mode, "cancel" | "teardown-timeout" | "slow-start") {
                cancel.store(true, Ordering::Relaxed);
            }
            if mode == "device" {
                failed.store(true, Ordering::Release);
            }
            if mode == "malformed" {
                control
                    .send_to(
                        &[0x80, 0xd5, 0, 1, 0, 0, 0, 33],
                        ("127.0.0.1", local_control),
                    )
                    .unwrap();
            }
            // Keep the negotiated endpoints alive through the failure.
            while data.recv(&mut packet).is_ok() {
                if mode == "duration" {
                    delay.sequence = delay.sequence.wrapping_add(1);
                    peer_event.send(&delay.encode().unwrap()).unwrap();
                }
            }
        });
        let started = Instant::now();
        let result = if live {
            let capture_closed = closed.clone();
            owner.play_pcm(
                None,
                Some((
                    Box::new(move || {
                        opened.store(true, Ordering::Release);
                        crate::audio::live::Capture::spawn(move || {
                            if mode == "startup" {
                                anyhow::bail!("mock capture open failed");
                            }
                            Ok(crate::audio::live::tests::Mock {
                                closed: capture_closed,
                                reads: 0,
                                fail_after: None,
                                pending: matches!(mode, "startup-cancel" | "startup-disconnect"),
                                failure: Some(failure),
                                gate: (mode == "slow-start").then_some(gate),
                            })
                        })
                    }),
                    (mode == "duration").then_some(1),
                )),
                None,
            )
        } else {
            owner.play_wav(&vec![[123, -456]; 4 * profile::RATE as usize], None)
        };
        let _ = finished.send(());
        if mode == "teardown-timeout" {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "TEARDOWN exceeded cleanup bound"
            );
        }
        assert!(
            started.elapsed() < Duration::from_secs(if mode == "slow-start" { 7 } else { 5 }),
            "unbounded playback/cleanup: {mode}"
        );
        if live && mode != "startup" {
            assert!(closed.load(Ordering::Acquire));
        }
        if mode == "duration" {
            result.unwrap();
            owner.stop().unwrap();
            assert_eq!(owner.state(), State::Closed);
        } else {
            assert!(result.is_err(), "{mode}");
            assert_eq!(owner.state(), State::Failed);
        }
        if matches!(mode, "startup" | "startup-cancel" | "startup-disconnect") {
            assert_eq!(owner.playback_stats.packets, 0);
        } else {
            assert!(owner.playback_stats.packets > 0);
        }
        assert!(owner.playback_stats.packets < 4 * u64::from(profile::RATE) / profile::SPF as u64);
        assert!(owner.control.is_none() && owner.timing.is_none());
        assert!(owner.realtime_encoder.is_none() && owner.realtime_transport.is_none());
        let _rebound = UdpSocket::bind(("127.0.0.1", local_control)).unwrap();
        let _event = UdpSocket::bind(addresses.0).unwrap();
        let _general = UdpSocket::bind(addresses.1).unwrap();
        receiver.join().unwrap();
        server.join().unwrap();
    }
}

#[test]
fn wav_cannot_send_without_authenticated_negotiated_owner() {
    let mut owner = Session::new();
    assert!(owner.play_wav(&[[1, -1]; 352], None).is_err());
    assert_eq!(owner.state(), State::Failed);
    assert_eq!(owner.playback_stats.packets, 0);
    assert_eq!(owner.playback_stats.syncs, 0);
    assert!(owner.control.is_none() && owner.realtime_transport.is_none());
}

#[test]
fn local_ptp_bind_permission_failure_tears_down_accepted_setup() {
    for port in [319, 320] {
        let (mut owner, _approval, server) = fixture(|mut socket| {
            let mut keys = server_keys();
            read_request(&mut socket, &mut keys);
            reply(&mut socket, &mut keys, 1, Some(info()));
            read_request(&mut socket, &mut keys);
            reply(&mut socket, &mut keys, 2, Some(setup(123)));
            let (head, _) = read_request(&mut socket, &mut keys);
            assert!(head.starts_with("TEARDOWN "));
            reply(&mut socket, &mut keys, 3, None);
            assert_eq!(socket.read(&mut [0]).unwrap(), 0);
        });
        let result = owner.negotiate_with(|_, _, _| {
            Err(super::super::wire::ptp_bind_error(
                port,
                std::io::Error::from_raw_os_error(libc::EACCES),
            ))
        });
        let error = owner.finish(result).unwrap_err();
        assert!(error.to_string().contains(&format!("local PTP UDP {port}")));
        assert!(error
            .downcast_ref::<super::super::wire::ResponseFailure>()
            .is_none());
        assert_eq!(owner.state(), State::Failed);
        assert!(!owner.session_bound);
        assert!(owner.control.is_none() && owner.timing.is_none() && owner.events.is_none());
        assert!(owner.diagnostic().contains("TEARDOWN: acknowledged"));
        server.join().unwrap();
    }
}
