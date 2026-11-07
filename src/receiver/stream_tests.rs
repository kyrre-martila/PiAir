//! Receiver-side assertions over the actual HAP/control/UDP/NTP session. Loopback only.
use super::*;
use crate::receiver::{
    profile,
    session::{Session, State},
};
use plist::Value;
use std::{
    collections::BTreeMap,
    net::UdpSocket,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};
struct Request {
    method: String,
    uri: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}
fn read_frame(socket: &mut TcpStream, keys: &mut ControlKeys) -> Vec<u8> {
    let mut length = [0; 2];
    socket.read_exact(&mut length).unwrap();
    let size = usize::from(u16::from_le_bytes(length));
    assert!((1..=1024).contains(&size));
    let mut frame = vec![0; size + 18];
    frame[..2].copy_from_slice(&length);
    socket.read_exact(&mut frame[2..]).unwrap();
    keys.decrypt(&frame).unwrap().to_vec()
}
fn read_message(socket: &mut TcpStream, keys: &mut ControlKeys) -> Request {
    let mut raw = read_frame(socket, keys);
    let head = loop {
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        assert!(raw.len() < 8192);
        raw.extend(read_frame(socket, keys));
    };
    let text = String::from_utf8(raw[..head].to_vec()).unwrap();
    let mut lines = text[..head - 4].split("\r\n");
    let words: Vec<_> = lines.next().unwrap().split(' ').collect();
    assert_eq!(words.len(), 3);
    assert_eq!(words[2], "RTSP/1.0");
    let headers: BTreeMap<_, _> = lines
        .map(|l| {
            let (k, v) = l.split_once(':').unwrap();
            (k.to_ascii_lowercase(), v.trim().to_owned())
        })
        .collect();
    let length = headers["content-length"].parse::<usize>().unwrap();
    assert!(length <= 8192);
    while raw.len() < head + length {
        raw.extend(read_frame(socket, keys));
    }
    assert_eq!(raw.len(), head + length);
    Request {
        method: words[0].into(),
        uri: words[1].into(),
        headers,
        body: raw[head..].to_vec(),
    }
}
fn send_reply(
    socket: &mut TcpStream,
    keys: &mut ControlKeys,
    r: &Request,
    value: Option<Value>,
    extra: &str,
) {
    let mut body = Vec::new();
    if let Some(v) = value {
        v.to_writer_binary(&mut body).unwrap();
    }
    let mut message=format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\nContent-Length: {}\r\nSession: test-session;timeout=60\r\n{}{}\r\n",r.headers["cseq"],body.len(),if body.is_empty(){""}else{"Content-Type: application/x-apple-binary-plist\r\n"},extra).into_bytes();
    message.extend_from_slice(&body);
    for part in message.chunks(127) {
        socket.write_all(&keys.encrypt(part).unwrap()).unwrap();
    }
}
fn plist(r: &Request) -> Value {
    Value::from_reader(std::io::Cursor::new(&r.body)).unwrap()
}
#[derive(Clone, Copy)]
enum Mode {
    Good,
    BadCodec,
    BadPort,
    NoTiming,
    NoEvent,
    BadTiming,
    SilentTeardown,
    EventRefused,
    BadEventTag,
    Revoke,
    Disconnect,
    BadSession,
    BadFeedback,
}
fn receiver(s: TcpStream, mode: Mode, seen: Arc<AtomicU64>) {
    let mut shared = [0; 32];
    let (mut socket, mut keys) = verify_mock_core(s, Fault::None, PI, |key| shared = *key).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let info = read_message(&mut socket, &mut keys);
    assert_eq!((&*info.method, &*info.uri), ("GET", "/info"));
    let features = if matches!(mode, Mode::BadCodec) {
        1 << 9
    } else {
        (1 << 9) | (1 << 19) | (1 << 46)
    };
    send_reply(
        &mut socket,
        &mut keys,
        &info,
        Some(profile::dict([("features", profile::uint(features))])),
        "",
    );
    if matches!(mode, Mode::BadCodec) {
        assert_eof(&mut socket);
        return;
    }
    let base = read_message(&mut socket, &mut keys);
    assert_eq!(base.method, "SETUP");
    assert!(base.uri.starts_with("rtsp://127.0.0.1/"));
    let base_value = plist(&base);
    let base_d = base_value.as_dictionary().unwrap();
    assert_eq!(base_d["timingProtocol"].as_string(), Some("NTP"));
    assert!(uuid::Uuid::parse_str(base_d["sessionUUID"].as_string().unwrap()).is_ok());
    let destination = base_d["timingPort"].as_unsigned_integer().unwrap() as u16;
    let event = TcpListener::bind("127.0.0.1:0").unwrap();
    let event_port = event.local_addr().unwrap().port();
    let timing = UdpSocket::bind("127.0.0.1:0").unwrap();
    let timing_port = timing.local_addr().unwrap().port();
    let data = UdpSocket::bind("127.0.0.1:0").unwrap();
    let data_port = data.local_addr().unwrap().port();
    let control = Arc::new(UdpSocket::bind("127.0.0.1:0").unwrap());
    let control_port = control.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let time_stop = stop.clone();
    let timer = thread::spawn(move || {
        timing.connect(("127.0.0.1", destination)).unwrap();
        timing
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut query = [0; 32];
        query[..4].copy_from_slice(&[0x80, 0xd2, 0, 7]);
        query[24..].copy_from_slice(&123_u64.to_be_bytes());
        if matches!(mode, Mode::BadTiming) {
            query[1] = 0;
        }
        while !time_stop.load(Ordering::Relaxed) {
            if !matches!(mode, Mode::NoTiming | Mode::SilentTeardown) {
                timing.send(&query).unwrap();
                let mut b = [0; 64];
                match timing.recv(&mut b) {
                    Ok(n) => {
                        assert_eq!(n, 32);
                        assert_eq!(&b[..4], &[0x80, 0xd3, 0, 7]);
                        assert_eq!(&b[8..16], &query[24..]);
                        assert!(
                            u64::from_be_bytes(b[24..32].try_into().unwrap())
                                >= u64::from_be_bytes(b[16..24].try_into().unwrap())
                        );
                    }
                    Err(e)
                        if [
                            std::io::ErrorKind::WouldBlock,
                            std::io::ErrorKind::TimedOut,
                            std::io::ErrorKind::ConnectionRefused,
                        ]
                        .contains(&e.kind()) => {}
                    Err(e) => panic!("{e}"),
                }
            }
            thread::park_timeout(Duration::from_millis(100));
        }
    });
    let event = if matches!(mode, Mode::EventRefused) {
        drop(event);
        None
    } else {
        Some(event)
    };
    send_reply(
        &mut socket,
        &mut keys,
        &base,
        Some(profile::dict([
            ("eventPort", profile::uint(u64::from(event_port))),
            ("timingPort", profile::uint(u64::from(timing_port))),
        ])),
        "",
    );
    if matches!(mode, Mode::EventRefused) {
        let end = read_message(&mut socket, &mut keys);
        assert_eq!(end.method, "TEARDOWN");
        send_reply(&mut socket, &mut keys, &end, None, "");
        stop.store(true, Ordering::Relaxed);
        timer.join().unwrap();
        assert_eof(&mut socket);
        return;
    }
    let event = event.unwrap();
    let event_stop = stop.clone();
    let events = thread::spawn(move || {
        let (mut socket, _) = event.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        if matches!(mode, Mode::NoEvent) {
            while !event_stop.load(Ordering::Relaxed) {
                thread::park_timeout(Duration::from_millis(20));
            }
            assert_eof(&mut socket);
            return;
        }
        let mut keys = ControlKeys::events(&shared);
        keys.swap_directions_for_test();
        let mut body = Vec::new();
        profile::dict([
            ("type", Value::String("updateInfo".into())),
            ("value", profile::dict([])),
        ])
        .to_writer_binary(&mut body)
        .unwrap();
        let mut message=format!("POST /command RTSP/1.0\r\nCSeq: 0\r\nContent-Length: {}\r\nContent-Type: application/x-apple-binary-plist\r\n\r\n",body.len()).into_bytes();
        message.extend_from_slice(&body);
        let mut frame = keys.encrypt(&message).unwrap();
        if matches!(mode, Mode::BadEventTag) {
            frame[3] ^= 1;
        }
        socket.write_all(&frame).unwrap();
        if matches!(mode, Mode::BadEventTag) {
            assert_eof(&mut socket);
            return;
        }
        let ack = read_frame(&mut socket, &mut keys);
        assert!(ack.starts_with(b"RTSP/1.0 200 OK\r\nCSeq: 0\r\n"));
        // A healthy event channel stays open until local teardown, with no fake events.
        while !event_stop.load(Ordering::Relaxed) {
            thread::park_timeout(Duration::from_millis(20));
        }
    });
    let record = read_message(&mut socket, &mut keys);
    assert_eq!(record.method, "RECORD");
    assert!(record.body.is_empty());
    assert_eq!(record.uri, base.uri);
    send_reply(
        &mut socket,
        &mut keys,
        &record,
        None,
        "Audio-Latency: 11025\r\n",
    );
    let stream = read_message(&mut socket, &mut keys);
    assert_eq!(stream.method, "SETUP");
    let offer = plist(&stream);
    let stream_d = offer.as_dictionary().unwrap()["streams"]
        .as_array()
        .unwrap()[0]
        .as_dictionary()
        .unwrap();
    for (k, v) in [
        ("type", 96),
        ("audioFormat", 0x40000),
        ("ct", 2),
        ("spf", 352),
        ("sr", 44100),
        ("latencyMin", 11025),
        ("latencyMax", 88200),
    ] {
        assert_eq!(stream_d[k].as_unsigned_integer(), Some(v));
    }
    let media_key: [u8; 32] = stream_d["shk"].as_data().unwrap().try_into().unwrap();
    assert_ne!(media_key, shared);
    let client_control = stream_d["controlPort"].as_unsigned_integer().unwrap() as u16;
    if matches!(mode, Mode::BadSession) {
        let response = format!(
            "RTSP/1.0 200 OK\r\nCSeq: {}\r\nContent-Length: 0\r\nSession: changed\r\n\r\n",
            stream.headers["cseq"]
        );
        socket
            .write_all(&keys.encrypt(response.as_bytes()).unwrap())
            .unwrap();
        let end = read_message(&mut socket, &mut keys);
        assert_eq!(end.method, "TEARDOWN");
        send_reply(&mut socket, &mut keys, &end, None, "");
        stop.store(true, Ordering::Relaxed);
        timer.join().unwrap();
        events.join().unwrap();
        assert_eof(&mut socket);
        return;
    }
    send_reply(
        &mut socket,
        &mut keys,
        &stream,
        Some(profile::dict([(
            "streams",
            Value::Array(vec![profile::dict([
                ("type", profile::uint(96)),
                (
                    "dataPort",
                    profile::uint(if matches!(mode, Mode::BadPort) {
                        65536
                    } else {
                        u64::from(data_port)
                    }),
                ),
                ("controlPort", profile::uint(u64::from(control_port))),
                ("streamID", profile::uint(1)),
            ])]),
        )])),
        "",
    );
    let sync_count = Arc::new(AtomicU64::new(0));
    let sync_seen = sync_count.clone();
    let sync_stop = stop.clone();
    let sync_socket = control.clone();
    let sync_thread = thread::spawn(move || {
        sync_socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut b = [0; 64];
        let mut previous_ntp = None;
        while !sync_stop.load(Ordering::Relaxed) {
            match sync_socket.recv_from(&mut b) {
                Ok((n, _)) => {
                    assert_eq!(n, 20);
                    assert_eq!(b[1], 0xd4);
                    assert_eq!(&b[2..4], &[0, 7]);
                    let ordinal = sync_seen.fetch_add(1, Ordering::Relaxed);
                    assert_eq!(b[0], if ordinal == 0 { 0x90 } else { 0x80 });
                    let current = u32::from_be_bytes(b[4..8].try_into().unwrap());
                    let next = u32::from_be_bytes(b[16..20].try_into().unwrap());
                    assert_eq!(next.wrapping_sub(current), 77175);
                    let ntp = u64::from_be_bytes(b[8..16].try_into().unwrap());
                    if let Some(old) = previous_ntp {
                        assert!(ntp > old);
                    }
                    previous_ntp = Some(ntp);
                }
                Err(e)
                    if [std::io::ErrorKind::WouldBlock, std::io::ErrorKind::TimedOut]
                        .contains(&e.kind()) => {}
                Err(e) => panic!("{e}"),
            }
        }
    });
    let media_stop = stop.clone();
    let seen_clone = seen.clone();
    let closer = socket.try_clone().unwrap();
    let data_thread = thread::spawn(move || {
        data.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut first_packet = None;
        let mut previous: Option<(u16, u32, u64, u32)> = None;
        let mut index = 0u64;
        let mut b = [0; 2048];
        while !media_stop.load(Ordering::Relaxed) {
            match data.recv_from(&mut b) {
                Ok((n, _)) => {
                    let packet = &b[..n];
                    if first_packet.as_deref() == Some(packet) {
                        continue;
                    }
                    assert!(n > 36);
                    assert_eq!(packet[0], 0x80);
                    let seq = u16::from_be_bytes(packet[2..4].try_into().unwrap());
                    let ts = u32::from_be_bytes(packet[4..8].try_into().unwrap());
                    let ssrc = u32::from_be_bytes(packet[8..12].try_into().unwrap());
                    let nonce: [u8; 8] = packet[n - 8..].try_into().unwrap();
                    let counter = u64::from_le_bytes(nonce);
                    if let Some((old_seq, old_ts, old_nonce, old_ssrc)) = previous {
                        assert_eq!(seq, old_seq.wrapping_add(1));
                        assert_eq!(ts, old_ts.wrapping_add(352));
                        assert_eq!(counter, old_nonce + 1);
                        assert_eq!(ssrc, old_ssrc);
                        assert_eq!(packet[1], 0x60);
                    } else {
                        assert_eq!(packet[1], 0xe0);
                        assert_eq!(counter, 0);
                    }
                    let payload = open(
                        &media_key,
                        &nonce_with_zero_prefix(&nonce),
                        &packet[12..n - 8],
                        &packet[4..12],
                    )
                    .unwrap();
                    let samples = crate::audio::alac::tests::decode_at_rate(&payload, 44100);
                    assert_eq!(samples.len(), 352);
                    for sample in samples {
                        let expected = if index >= 44100 {
                            0
                        } else {
                            let phase = (index % 100) as i16;
                            if phase < 50 {
                                phase * 164 - 4100
                            } else {
                                (100 - phase) * 164 - 4100
                            }
                        };
                        assert_eq!(sample, [expected, expected]);
                        index += 1;
                    }
                    if previous.is_none() {
                        first_packet = Some(packet.to_vec());
                        let mut request = [0x80, 0xd5, 0, 1, 0, 0, 0, 1];
                        request[4..6].copy_from_slice(&seq.to_be_bytes());
                        control
                            .send_to(&request, ("127.0.0.1", client_control))
                            .unwrap();
                    }
                    previous = Some((seq, ts, counter, ssrc));
                    let count = seen_clone.fetch_add(1, Ordering::Relaxed) + 1;
                    if matches!(mode, Mode::Disconnect) && count == 10 {
                        closer.shutdown(std::net::Shutdown::Both).unwrap();
                    }
                }
                Err(e)
                    if [std::io::ErrorKind::WouldBlock, std::io::ErrorKind::TimedOut]
                        .contains(&e.kind()) => {}
                Err(e) => panic!("{e}"),
            }
        }
    });
    loop {
        let mut first = [0];
        match socket.peek(&mut first) {
            Ok(0) => {
                assert!(matches!(mode, Mode::Revoke | Mode::Disconnect));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {
                assert!(matches!(mode, Mode::Revoke | Mode::Disconnect));
                break;
            }
            Err(e) => panic!("{e}"),
            Ok(_) => (),
        }
        let r = read_message(&mut socket, &mut keys);
        assert_eq!(r.headers["session"], "test-session");
        assert_eq!(
            r.uri,
            if r.method == "POST" {
                "/feedback"
            } else {
                &base.uri
            }
        );
        match r.method.as_str() {
            "SET_PARAMETER" => {
                assert_eq!(r.headers["content-type"], "text/parameters");
                assert_eq!(r.body, b"volume: -30.000000\r\n");
                send_reply(&mut socket, &mut keys, &r, None, "");
            }
            "POST" => {
                send_reply(
                    &mut socket,
                    &mut keys,
                    &r,
                    Some(profile::dict([(
                        "streams",
                        Value::Array(if matches!(mode, Mode::BadFeedback) {
                            vec![profile::dict([
                                ("type", profile::uint(96)),
                                ("streamID", profile::uint(999)),
                            ])]
                        } else {
                            vec![]
                        }),
                    )])),
                    "",
                );
            }
            "FLUSH" => {
                assert!(r.headers["rtp-info"].starts_with("seq="));
                assert!(r.headers["rtp-info"].contains(";rtptime="));
                send_reply(&mut socket, &mut keys, &r, None, "");
            }
            "TEARDOWN" => {
                assert!(plist(&r).as_dictionary().unwrap().is_empty());
                if !matches!(mode, Mode::SilentTeardown) {
                    send_reply(&mut socket, &mut keys, &r, None, "");
                }
                break;
            }
            other => panic!("unexpected {other}"),
        }
    }
    stop.store(true, Ordering::Relaxed);
    timer.join().unwrap();
    events.join().unwrap();
    data_thread.join().unwrap();
    sync_thread.join().unwrap();
    if matches!(mode, Mode::Good) && seen.load(Ordering::Relaxed) > 0 {
        assert!(sync_count.load(Ordering::Relaxed) >= 3);
    }
    if !matches!(mode, Mode::Disconnect) {
        assert_eof(&mut socket);
    }
}
fn nonce_with_zero_prefix(nonce: &[u8; 8]) -> [u8; 12] {
    let mut n = [0; 12];
    n[4..].copy_from_slice(nonce);
    n
}
#[test]
fn full_authenticated_realtime_session_encodes_encrypts_synchronizes_and_tears_down() {
    let seen = Arc::new(AtomicU64::new(0));
    let count = seen.clone();
    let (wire, server) = local_mock(move |s| receiver(s, Mode::Good, count));
    let mut session = Session::new();
    session.start(|| Ok(wire), &credential()).unwrap();
    session.negotiate_realtime().unwrap();
    assert_eq!(session.state(), State::Ready);
    session.transmit_test(1).unwrap();
    assert_eq!(session.state(), State::Streaming);
    assert!(session.packets_sent > 350);
    assert!(session.resends > 0);
    assert!(session.timing_exchanges > 0);
    let sent = session.packets_sent;
    session.stop().unwrap();
    session.stop().unwrap();
    assert_eq!(session.state(), State::Disconnected);
    server.join().unwrap();
    assert_eq!(seen.load(Ordering::Relaxed), sent);
}
#[test]
fn unsupported_codec_and_partial_setup_errors_fail_and_cleanup() {
    for mode in [
        Mode::BadCodec,
        Mode::BadPort,
        Mode::NoTiming,
        Mode::NoEvent,
        Mode::BadTiming,
        Mode::SilentTeardown,
        Mode::EventRefused,
        Mode::BadEventTag,
        Mode::BadSession,
    ] {
        let seen = Arc::new(AtomicU64::new(0));
        let count = seen.clone();
        let (wire, server) = local_mock(move |s| receiver(s, mode, count));
        let mut session = Session::new();
        session.start(|| Ok(wire), &credential()).unwrap();
        let start = std::time::Instant::now();
        let error = session.negotiate_realtime().unwrap_err();
        let diagnostic = format!("{error:#}");
        match mode {
            Mode::NoTiming | Mode::SilentTeardown => assert!(
                diagnostic.contains("ntp_ready=false event_ready=true"),
                "{diagnostic}"
            ),
            Mode::EventRefused => assert!(
                diagnostic.contains("stage=event connection"),
                "{diagnostic}"
            ),
            Mode::NoEvent => assert!(
                diagnostic.contains("ntp_ready=true event_ready=false"),
                "{diagnostic}"
            ),
            Mode::BadEventTag => assert!(diagnostic.contains("event worker"), "{diagnostic}"),
            Mode::BadTiming => assert!(diagnostic.contains("NTP worker"), "{diagnostic}"),
            Mode::BadPort | Mode::BadSession => assert!(
                diagnostic.contains("stage=stream SETUP") && diagnostic.contains("status=200"),
                "{diagnostic}"
            ),
            Mode::BadCodec => assert!(
                diagnostic.contains("stage=GET /info") && diagnostic.contains("status=200"),
                "{diagnostic}"
            ),
            _ => unreachable!(),
        }
        assert_eq!(session.failure(), Some(diagnostic.as_str()));
        assert!(start.elapsed() < Duration::from_secs(5));
        assert_eq!(session.state(), State::Failed);
        assert_eq!(session.packets_sent, 0);
        session.stop().unwrap();
        server.join().unwrap();
        assert_eq!(seen.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn live_approval_revocation_stops_new_media_and_does_not_send_teardown_to_revoked_peer() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, format!("receiver_allowlist=['{ID}']")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let selected = selection(port);
    store::Store::new(&config)
        .unwrap()
        .select(&selected)
        .unwrap();
    let seen = Arc::new(AtomicU64::new(0));
    let count = seen.clone();
    let server = thread::spawn(move || receiver(listener.accept().unwrap().0, Mode::Revoke, count));
    let mut session = Session::new();
    session
        .start(
            || Wire::connect(&config, ID, &selected, &snapshot(port)),
            &credential(),
        )
        .unwrap();
    session.negotiate_realtime().unwrap();
    std::fs::write(&config, "").unwrap();
    assert!(session.transmit_test(1).is_err());
    assert_eq!(session.state(), State::Failed);
    assert_eq!(session.packets_sent, 0);
    assert!(session.failure().unwrap().contains("revoked"));
    session.stop().unwrap();
    server.join().unwrap();
    assert_eq!(seen.load(Ordering::Relaxed), 0);
}
#[test]
fn interrupted_receiver_fails_without_reconnect() {
    let seen = Arc::new(AtomicU64::new(0));
    let (wire, server) = local_mock(move |s| receiver(s, Mode::Disconnect, seen));
    let mut session = Session::new();
    session.start(|| Ok(wire), &credential()).unwrap();
    session.negotiate_realtime().unwrap();
    assert!(session.transmit_test(1).is_err());
    assert_eq!(session.state(), State::Failed);
    assert!(session.packets_sent > 0);
    session.stop().unwrap();
    server.join().unwrap();
}
#[test]
fn same_owner_can_start_and_stop_fresh_authenticated_sessions() {
    let mut session = Session::new();
    for _ in 0..2 {
        let seen = Arc::new(AtomicU64::new(0));
        let (wire, server) = local_mock(move |s| receiver(s, Mode::Good, seen));
        session.start(|| Ok(wire), &credential()).unwrap();
        session.negotiate_realtime().unwrap();
        assert_eq!(session.state(), State::Ready);
        session.stop().unwrap();
        session.stop().unwrap();
        assert_eq!(session.state(), State::Disconnected);
        server.join().unwrap();
    }
}

#[test]
fn feedback_stream_identity_mismatch_fails_the_active_session() {
    let seen = Arc::new(AtomicU64::new(0));
    let (wire, server) = local_mock(move |s| receiver(s, Mode::BadFeedback, seen));
    let mut session = Session::new();
    session.start(|| Ok(wire), &credential()).unwrap();
    session.negotiate_realtime().unwrap();
    assert!(session.transmit_test(1).is_err());
    assert!(session.failure().unwrap().contains("stream identifier"));
    assert_eq!(session.state(), State::Failed);
    session.stop().unwrap();
    server.join().unwrap();
}

#[test]
fn cancelled_diagnostic_tears_down_without_sending_audio() {
    let seen = Arc::new(AtomicU64::new(0));
    let count = seen.clone();
    let (wire, server) = local_mock(move |s| receiver(s, Mode::Good, count));
    let mut session = Session::new();
    session.start(|| Ok(wire), &credential()).unwrap();
    session.negotiate_realtime().unwrap();
    session.cancel.store(true, Ordering::Relaxed);
    assert!(session.transmit_test(1).is_err());
    assert!(session.failure().unwrap().contains("interrupted"));
    assert_eq!(session.state(), State::Failed);
    assert_eq!(session.packets_sent, 0);
    session.stop().unwrap();
    server.join().unwrap();
    assert_eq!(seen.load(Ordering::Relaxed), 0);
}

#[derive(Clone, Copy, Debug)]
enum NegotiationFault {
    Timeout,
    Status,
    Malformed,
}

#[test]
fn request_failures_attribute_each_negotiation_stage_and_close_channels() {
    for (target, stage, method) in [
        (0, "GET /info", "GET"),
        (1, "initial SETUP", "SETUP"),
        (2, "RECORD", "RECORD"),
        (3, "stream SETUP", "SETUP"),
        (4, "SET_PARAMETER volume", "SET_PARAMETER"),
    ] {
        for fault in [
            NegotiationFault::Timeout,
            NegotiationFault::Status,
            NegotiationFault::Malformed,
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let server = thread::spawn(move || {
                let (mut socket, mut keys) =
                    verify_mock_core(listener.accept().unwrap().0, Fault::None, PI, |_| {})
                        .unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let event = TcpListener::bind("127.0.0.1:0").unwrap();
                let timing = UdpSocket::bind("127.0.0.1:0").unwrap();
                let data = UdpSocket::bind("127.0.0.1:0").unwrap();
                let mut event_socket = None;
                for index in 0..=target {
                    let request = read_message(&mut socket, &mut keys);
                    assert_eq!(
                        request.method,
                        ["GET", "SETUP", "RECORD", "SETUP", "SET_PARAMETER"][index]
                    );
                    if index == target {
                        match fault {
                            NegotiationFault::Timeout => {} // Wait for the client's deadline and EOF.
                            NegotiationFault::Status | NegotiationFault::Malformed => {
                                // Deliberately sensitive reason/header/body must never appear in diagnostics.
                                let response = if matches!(fault, NegotiationFault::Status) {
                                    "RTSP/1.0 453 SECRET-REASON\r\nAuthorization: SECRET-HEADER\r\nContent-Length: 11\r\n\r\nSECRET-BODY".to_owned()
                                } else {
                                    "RTSP/1.0 200 OK\r\nCSeq: wrong-SECRET\r\nContent-Length: 0\r\n\r\n".to_owned()
                                };
                                socket
                                    .write_all(&keys.encrypt(response.as_bytes()).unwrap())
                                    .unwrap();
                            }
                        }
                        assert_eof(&mut socket);
                        if let Some(mut event) = event_socket {
                            assert_eof(&mut event);
                        }
                        return;
                    }
                    let value = match index {
                        0 => Some(profile::dict([(
                            "features",
                            profile::uint((1 << 9) | (1 << 19)),
                        )])),
                        1 => Some(profile::dict([
                            (
                                "eventPort",
                                profile::uint(u64::from(event.local_addr().unwrap().port())),
                            ),
                            (
                                "timingPort",
                                profile::uint(u64::from(timing.local_addr().unwrap().port())),
                            ),
                        ])),
                        3 => Some(profile::dict([(
                            "streams",
                            Value::Array(vec![profile::dict([
                                ("type", profile::uint(96)),
                                (
                                    "dataPort",
                                    profile::uint(u64::from(data.local_addr().unwrap().port())),
                                ),
                                (
                                    "controlPort",
                                    profile::uint(u64::from(data.local_addr().unwrap().port())),
                                ),
                            ])]),
                        )])),
                        _ => None,
                    };
                    send_reply(&mut socket, &mut keys, &request, value, "");
                    if index == 1 {
                        event_socket = Some(event.accept().unwrap().0);
                        event_socket
                            .as_ref()
                            .unwrap()
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                    }
                }
            });
            let wire = Wire::mock(socket, Duration::from_millis(300), ID);
            let mut session = Session::new();
            session.start(|| Ok(wire), &credential()).unwrap();
            let start = std::time::Instant::now();
            let error = session.negotiate_realtime().unwrap_err();
            let diagnostic = format!("{error:#}");
            assert!(
                diagnostic.contains(&format!("stage={stage}")),
                "{diagnostic}"
            );
            assert!(
                diagnostic.contains(&format!("method={method}")),
                "{diagnostic}"
            );
            match fault {
                NegotiationFault::Timeout => {
                    assert!(diagnostic.contains("read timeout"), "{diagnostic}")
                }
                NegotiationFault::Status => {
                    assert!(diagnostic.contains("status 453"), "{diagnostic}")
                }
                NegotiationFault::Malformed => assert!(
                    diagnostic.contains("status=200")
                        && diagnostic.contains("invalid receiver CSeq"),
                    "{diagnostic}"
                ),
            }
            assert!(!diagnostic.contains("SECRET"), "{diagnostic}");
            assert_eq!(session.failure(), Some(diagnostic.as_str()));
            assert_eq!(session.state(), State::Failed);
            assert_eq!(session.packets_sent, 0);
            session.stop().unwrap();
            session.stop().unwrap();
            assert!(start.elapsed() < Duration::from_secs(2));
            server.join().unwrap();
        }
    }
}
