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
fn timing_probe(destination: u16) {
    let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let mut query = [0; 32];
    query[..4].copy_from_slice(&[0x80, 0xd2, 0x12, 0x34]);
    query[24..].copy_from_slice(&123_u64.to_be_bytes());
    peer.send_to(&query, ("127.0.0.1", destination)).unwrap();
    let mut reply = [0; 33];
    let (n, source) = peer.recv_from(&mut reply).unwrap();
    assert_eq!(source.port(), destination);
    assert_eq!(n, 32);
    assert_eq!(&reply[..4], &[0x80, 0xd3, 0x12, 0x34]);
    assert_eq!(&reply[8..16], &query[24..]);
}
#[derive(Clone, Copy)]
enum Mode {
    Good,
    RecordLatency(Option<&'static str>),
    NoRemoteTiming,
    BadCodec,
    UnsupportedTiming,
    BadPort,
    NoTiming,
    NoEvent,
    RemoteCommand,
    UnknownEvent,
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
    let (socket, keys) = verify_mock_core(s, Fault::None, PI, |key| shared = *key).unwrap();
    receiver_authenticated(socket, keys, shared.to_vec(), mode, seen);
}
fn receiver_authenticated(
    mut socket: TcpStream,
    mut keys: ControlKeys,
    shared: Vec<u8>,
    mode: Mode,
    seen: Arc<AtomicU64>,
) {
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
    for key in [
        "client-instance",
        "dacp-id",
        "active-remote",
        "x-apple-client-name",
    ] {
        assert_eq!(base.headers[key], info.headers[key]);
    }
    assert_eq!(base.headers["client-instance"], base.headers["dacp-id"]);
    assert!(u64::from_str_radix(&base.headers["dacp-id"], 16).is_ok());
    assert!(base.headers["active-remote"].parse::<u32>().is_ok());
    assert_eq!(base.headers["x-apple-client-name"], "PiAir");
    assert_eq!(base.headers["x-apple-protocolversion"], "1");
    assert_eq!(
        base.headers["content-type"],
        "application/x-apple-binary-plist"
    );
    let base_value = plist(&base);
    let base_d = base_value.as_dictionary().unwrap();
    assert_eq!(base_d.len(), 4);
    assert_eq!(base_d["timingProtocol"].as_string(), Some("NTP"));
    assert!(uuid::Uuid::parse_str(base_d["sessionUUID"].as_string().unwrap()).is_ok());
    let destination = base_d["timingPort"].as_unsigned_integer().unwrap() as u16;
    // Compare the production offer with Python plistlib's independent fixture,
    // substituting only runtime-generated identity and bound port values.
    let mut expected = Value::from_reader(std::io::Cursor::new(include_bytes!(
        "../../tests/fixtures/initial-setup.plist"
    )))
    .unwrap();
    for key in ["deviceID", "sessionUUID", "timingPort"] {
        expected
            .as_dictionary_mut()
            .unwrap()
            .insert(key.into(), base_d[key].clone());
    }
    assert_eq!(base_value, expected);
    if matches!(mode, Mode::UnsupportedTiming) {
        send_reply(
            &mut socket,
            &mut keys,
            &base,
            Some(profile::dict([(
                "timingProtocol",
                Value::String("PTP".into()),
            )])),
            "",
        );
        let end = read_message(&mut socket, &mut keys);
        assert_eq!(end.method, "TEARDOWN");
        send_reply(&mut socket, &mut keys, &end, None, "");
        assert_eof(&mut socket);
        return;
    }
    let event = TcpListener::bind("127.0.0.1:0").unwrap();
    let event_port = event.local_addr().unwrap().port();
    let timing = UdpSocket::bind("127.0.0.1:0").unwrap();
    let timing_port = timing.local_addr().unwrap().port();
    if matches!(mode, Mode::Good | Mode::NoRemoteTiming) {
        // Withhold the initial response until the advertised responder answers.
        timing_probe(destination);
    }
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
    let mut response = profile::dict([("eventPort", profile::uint(u64::from(event_port)))]);
    if !matches!(mode, Mode::NoRemoteTiming) {
        response
            .as_dictionary_mut()
            .unwrap()
            .insert("timingPort".into(), profile::uint(u64::from(timing_port)));
    }
    send_reply(&mut socket, &mut keys, &base, Some(response), "");
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
    let event_keys = ControlKeys::events(&shared);
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
        let mut keys = event_keys;
        keys.swap_directions_for_test();
        let mut body = Vec::new();
        profile::dict([
            (
                "type",
                Value::String(
                    match mode {
                        Mode::RemoteCommand => "sendMediaRemoteCommand",
                        Mode::UnknownEvent => "unsupportedPlaybackCommand",
                        _ => "updateInfo",
                    }
                    .into(),
                ),
            ),
            (
                "value",
                if matches!(mode, Mode::RemoteCommand | Mode::UnknownEvent) {
                    Value::String("paus".into())
                } else {
                    profile::dict([])
                },
            ),
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
    if matches!(mode, Mode::Good | Mode::NoRemoteTiming) {
        timing_probe(destination);
    }
    assert!(record.body.is_empty());
    assert_eq!(record.uri, base.uri);
    send_reply(
        &mut socket,
        &mut keys,
        &record,
        None,
        &match mode {
            Mode::Good | Mode::BadPort => "Audio-Latency: 0\r\n".into(),
            Mode::RecordLatency(None) => String::new(),
            Mode::RecordLatency(Some(value)) => format!("Audio-Latency: {value}\r\n"),
            _ => "Audio-Latency: 11025\r\n".into(),
        },
    );
    let stream = read_message(&mut socket, &mut keys);
    assert_eq!(stream.method, "SETUP");
    if matches!(mode, Mode::Good | Mode::NoRemoteTiming) {
        timing_probe(destination);
    }
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
    assert_ne!(media_key.as_slice(), shared.as_slice());
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
                if !matches!(
                    mode,
                    Mode::NoTiming | Mode::BadTiming | Mode::SilentTeardown | Mode::Revoke
                ) {
                    timing_probe(destination);
                }
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
    if matches!(mode, Mode::Good | Mode::NoRemoteTiming) && seen.load(Ordering::Relaxed) > 0 {
        assert!(sync_count.load(Ordering::Relaxed) >= 3);
    }
    if !matches!(mode, Mode::Disconnect) {
        assert_eof(&mut socket);
        if matches!(mode, Mode::Good | Mode::NoRemoteTiming) {
            let rebound = UdpSocket::bind(("127.0.0.1", destination)).unwrap();
            drop(rebound);
        }
    }
}
fn nonce_with_zero_prefix(nonce: &[u8; 8]) -> [u8; 12] {
    let mut n = [0; 12];
    n[4..].copy_from_slice(nonce);
    n
}
#[test]
fn full_authenticated_realtime_session_encodes_encrypts_synchronizes_and_tears_down() {
    for mode in [
        Mode::Good,
        Mode::NoRemoteTiming,
        Mode::NoEvent,
        Mode::RemoteCommand,
        Mode::UnknownEvent,
    ] {
        let seen = Arc::new(AtomicU64::new(0));
        let count = seen.clone();
        let (wire, server) = local_mock(move |s| receiver(s, mode, count));
        let mut session = Session::new();
        session.start(|| Ok(wire), &credential()).unwrap();
        session.negotiate_realtime().unwrap();
        assert_eq!(session.state(), State::Ready);
        assert_eq!(session.packets_sent, 0);
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
}
#[test]
fn informational_record_latency_always_proceeds_to_strict_stream_setup() {
    for value in [
        None,
        Some("0"),
        Some("11025"),
        Some("88200"),
        Some("1"),
        Some("88201"),
        Some("4294967295"),
        Some("4294967296"),
        Some("secret-token"),
        Some("-1"),
    ] {
        let seen = Arc::new(AtomicU64::new(0));
        let count = seen.clone();
        let (wire, server) = local_mock(move |s| receiver(s, Mode::RecordLatency(value), count));
        let mut session = Session::new();
        session.start(|| Ok(wire), &credential()).unwrap();
        session.negotiate_realtime().unwrap();
        assert_eq!(session.state(), State::Ready);
        session.stop().unwrap();
        server.join().unwrap();
        assert_eq!(seen.load(Ordering::Relaxed), 0);
    }
}
#[test]
fn unsupported_codec_and_partial_setup_errors_fail_and_cleanup() {
    for mode in [
        Mode::BadCodec,
        Mode::UnsupportedTiming,
        Mode::BadPort,
        Mode::NoTiming,
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
            Mode::UnsupportedTiming => {
                assert!(diagnostic.contains("unsupported PTP/timing profile"))
            }
            Mode::NoTiming | Mode::SilentTeardown | Mode::BadTiming => {
                assert!(diagnostic.contains("ntp_ready=false"), "{diagnostic}")
            }
            Mode::EventRefused => assert!(
                diagnostic.contains("stage=event connection"),
                "{diagnostic}"
            ),
            Mode::BadEventTag => assert!(diagnostic.contains("event worker"), "{diagnostic}"),
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
            let mut outcome = None;
            let cleanup_log = super::diagnostics::tests::capture(tracing::Level::DEBUG, || {
                outcome = Some(session.negotiate_realtime());
            });
            let error = outcome.unwrap().unwrap_err();
            if target > 0 {
                assert!(
                    cleanup_log
                        .contains("TEARDOWN skipped: control connection already invalidated"),
                    "target={target} error={error:#} log={cleanup_log}"
                );
                assert!(!cleanup_log.contains("session cleanup failed"));
            }
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

#[test]
fn transient_session_negotiates_encrypted_audio_and_acknowledges_teardown() {
    let seen = Arc::new(AtomicU64::new(0));
    let count = seen.clone();
    let (wire, server) = local_mock_method(Method::Transient, move |s| {
        setup_mock_with_transient(
            s,
            Fault::None,
            Method::Transient,
            "3939",
            PI,
            |socket, shared| {
                let mut keys = ControlKeys::new(shared.as_ref());
                keys.swap_directions_for_test();
                receiver_authenticated(socket, keys, shared.to_vec(), Mode::Good, count);
            },
        );
    });
    let mut session = Session::new();
    session.start_selected(|| Ok(wire), None).unwrap();
    session.negotiate_realtime().unwrap();
    session.transmit_test(1).unwrap();
    let sent = session.packets_sent;
    session.stop().unwrap();
    assert_eq!(session.state(), State::Disconnected);
    server.join().unwrap();
    assert!(sent > 350);
    assert_eq!(seen.load(Ordering::Relaxed), sent);
}

#[test]
fn initial_setup_check_validates_then_tears_down_without_record_or_audio() {
    for timeout in [None, Some(Duration::from_secs(15))] {
        initial_setup_check(
            timeout,
            profile::dict([
                ("eventPort", profile::uint(7001)),
                ("timingPort", profile::uint(7002)),
            ]),
            None,
        );
    }
}

#[test]
fn initial_ntp_response_contract() {
    let valid = profile::dict([("eventPort", profile::uint(7001))]);
    initial_setup_check(None, valid, None);

    initial_setup_check(None, profile::dict([]), Some("eventPort"));
    for value in [
        profile::uint(0),
        profile::uint(65536),
        Value::Integer((-1i64).into()),
        Value::String("7001".into()),
        Value::Boolean(true),
        Value::Real(7001.0),
    ] {
        initial_setup_check(
            None,
            profile::dict([("eventPort", value.clone())]),
            Some("eventPort"),
        );
        initial_setup_check(
            None,
            profile::dict([("eventPort", profile::uint(7001)), ("timingPort", value)]),
            Some("timingPort"),
        );
    }
    for protocol in [
        Value::String("PTP".into()),
        Value::String("None".into()),
        profile::uint(1),
    ] {
        initial_setup_check(
            None,
            profile::dict([
                ("eventPort", profile::uint(7001)),
                ("timingProtocol", protocol),
            ]),
            Some("unsupported PTP/timing profile"),
        );
    }
}

fn initial_setup_check(timeout: Option<Duration>, response: Value, error: Option<&str>) {
    let (wire, server) = local_mock(move |s| {
        let (mut socket, mut keys) = verify_mock_core(s, Fault::None, PI, |_| {}).unwrap();
        let info = read_message(&mut socket, &mut keys);
        assert_eq!((info.method.as_str(), info.uri.as_str()), ("GET", "/info"));
        send_reply(
            &mut socket,
            &mut keys,
            &info,
            Some(profile::dict([(
                "features",
                profile::uint((1 << 9) | (1 << 19) | (1 << 41)),
            )])),
            "",
        );
        let setup = read_message(&mut socket, &mut keys);
        assert_eq!(setup.method, "SETUP");
        assert!(setup.uri.starts_with("rtsp://127.0.0.1/"));
        assert_eq!(
            setup.headers["cseq"].parse::<u32>().unwrap(),
            info.headers["cseq"].parse::<u32>().unwrap() + 1
        );
        let value = plist(&setup);
        let d = value.as_dictionary().unwrap();
        assert_eq!(
            d["deviceID"].as_string().unwrap().replace(':', ""),
            format!(
                "{:016X}",
                u64::from_str_radix(&setup.headers["client-instance"], 16).unwrap()
            )
        );
        let uuid = uuid::Uuid::parse_str(d["sessionUUID"].as_string().unwrap()).unwrap();
        assert_eq!(uuid.get_version_num(), 4);
        send_reply(&mut socket, &mut keys, &setup, Some(response), "");
        let teardown = read_message(&mut socket, &mut keys);
        assert_eq!(teardown.method, "TEARDOWN");
        assert_eq!(teardown.uri, setup.uri);
        send_reply(&mut socket, &mut keys, &teardown, None, "");
        assert_eof(&mut socket);
    });
    let mut session = Session::new();
    session.start(|| Ok(wire), &credential()).unwrap();
    let result = session.check_initial_setup_with_timeout(timeout);
    if let Some(expected) = error {
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains(expected), "{error}");
        assert_eq!(session.state(), State::Failed);
        session.stop().unwrap();
        assert_eq!(session.state(), State::Failed);
    } else {
        result.unwrap();
        assert_eq!(session.state(), State::Disconnected);
    }
    assert_eq!(session.packets_sent, 0);
    server.join().unwrap();
}

// The same actual verify/control implementation drives every profile; the mock
// withholds SETUP until a real UDP reply, exposing the lifecycle difference.
#[test]
fn setup_profiles_order_crypto_and_readiness() {
    for selected in [SetupProfile::A, SetupProfile::B, SetupProfile::C] {
        let (wire, server) = local_mock(move |mut socket| {
            if selected == SetupProfile::C {
                preverify_info_mock(&mut socket);
            }
            let offset = u32::from(selected == SetupProfile::C);
            let (mut socket, mut keys) =
                verify_mock_core_offset(socket, Fault::None, PI, |_| {}, offset).unwrap();
            if selected != SetupProfile::C {
                let info = read_message(&mut socket, &mut keys);
                assert_eq!(info.method, "GET");
                assert_eq!(info.headers["cseq"], "3");
                send_reply(
                    &mut socket,
                    &mut keys,
                    &info,
                    Some(profile::dict([(
                        "features",
                        profile::uint((1 << 9) | (1 << 19) | (1 << 41)),
                    )])),
                    "",
                );
            }
            // Decrypting here requires TX nonce 1 for A/B, 0 for C.
            let setup = read_message(&mut socket, &mut keys);
            assert_eq!(setup.method, "SETUP");
            assert_eq!(setup.headers["cseq"], "4");
            let offer = plist(&setup);
            let port = profile::port(offer.as_dictionary().unwrap(), "timingPort").unwrap();
            let timing = UdpSocket::bind("127.0.0.1:0").unwrap();
            timing
                .set_read_timeout(Some(Duration::from_millis(250)))
                .unwrap();
            let mut query = [0; 32];
            query[..2].copy_from_slice(&[0x80, 0xd2]);
            query[24..].copy_from_slice(&1234u64.to_be_bytes());
            timing.send_to(&query, ("127.0.0.1", port)).unwrap();
            let mut reply = [0; 33];
            if selected == SetupProfile::A {
                assert!(timing.recv_from(&mut reply).is_err());
            } else {
                let (n, source) = timing.recv_from(&mut reply).unwrap();
                assert_eq!(n, 32);
                assert_eq!(source.port(), port);
                assert_eq!(&reply[..2], &[0x80, 0xd3]);
                assert_eq!(&reply[8..16], &query[24..32]);
                assert_ne!(&reply[16..24], &[0; 8]);
            }
            send_reply(
                &mut socket,
                &mut keys,
                &setup,
                Some(profile::dict([
                    ("timingProtocol", Value::String("NTP".into())),
                    ("eventPort", profile::uint(7001)),
                ])),
                "",
            );
            let teardown = read_message(&mut socket, &mut keys);
            assert_eq!(teardown.method, "TEARDOWN");
            assert_eq!(teardown.headers["cseq"], "5");
            send_reply(&mut socket, &mut keys, &teardown, None, "");
            assert_eof(&mut socket);
            // EOF follows synchronous joining and release of the bound sockets.
            let rebound = UdpSocket::bind(("127.0.0.1", port)).unwrap();
            drop(rebound);
        });
        let mut session = Session::new();
        session
            .start_setup_selected(|| Ok(wire), Some(&credential()), selected)
            .unwrap();
        session.check_initial_setup_with_timeout(None).unwrap();
        assert_eq!(session.state(), State::Disconnected);
        assert_eq!(session.packets_sent, 0);
        server.join().unwrap();
    }
}

fn preverify_info_mock(socket: &mut TcpStream) {
    let mut raw = Vec::new();
    while !raw.ends_with(b"\r\n\r\n") {
        let mut b = [0];
        socket.read_exact(&mut b).unwrap();
        raw.push(b[0]);
    }
    let text = String::from_utf8(raw).unwrap();
    assert!(text.starts_with("GET /info RTSP/1.0\r\nCSeq: 1\r\n"));
    let body = profile::binary(&mut profile::dict([(
        "features",
        profile::uint((1 << 9) | (1 << 19) | (1 << 41)),
    )]))
    .unwrap();
    write!(socket, "RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Type: application/x-apple-binary-plist\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
    socket.write_all(&body).unwrap();
}

#[test]
fn setup_profiles_cancel_timeout_and_failure_release_timing_socket() {
    let _ = super::diagnostics::tests::capture(tracing::Level::DEBUG, || {
        for (selected, playback) in [
            (SetupProfile::A, false),
            (SetupProfile::B, false),
            (SetupProfile::C, false),
            (SetupProfile::A, true),
        ] {
            for fault in ["cancel", "timeout", "invalid"] {
                if playback && fault == "cancel" {
                    continue;
                }
                let mut session = Session::new();
                let cancel = session.cancel.clone();
                let (cleaned_tx, cleaned_rx) = std::sync::mpsc::sync_channel(1);
                let (wire, server) = local_mock(move |mut socket| {
                    if selected == SetupProfile::C {
                        preverify_info_mock(&mut socket);
                    }
                    let offset = u32::from(selected == SetupProfile::C);
                    let (mut socket, mut keys) =
                        verify_mock_core_offset(socket, Fault::None, PI, |_| {}, offset).unwrap();
                    if selected != SetupProfile::C {
                        let info = read_message(&mut socket, &mut keys);
                        send_reply(
                            &mut socket,
                            &mut keys,
                            &info,
                            Some(profile::dict([(
                                "features",
                                profile::uint((1 << 9) | (1 << 19) | (1 << 41)),
                            )])),
                            "",
                        );
                    }
                    let setup = read_message(&mut socket, &mut keys);
                    let offer = plist(&setup);
                    let port = profile::port(offer.as_dictionary().unwrap(), "timingPort").unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(6)))
                        .unwrap();
                    if playback {
                        timing_probe(port);
                    }
                    match fault {
                        "cancel" => cancel.store(true, Ordering::Relaxed),
                        "invalid" => {
                            // A valid encrypted reply with invalid plist semantics must
                            // release the live timing worker too, after best-effort teardown.
                            send_reply(&mut socket, &mut keys, &setup, Some(profile::dict([])), "");
                            let teardown = read_message(&mut socket, &mut keys);
                            assert_eq!(teardown.method, "TEARDOWN");
                            if playback {
                                timing_probe(port);
                            }
                            send_reply(&mut socket, &mut keys, &teardown, None, "");
                        }
                        _ => (),
                    }
                    assert_eof(&mut socket);
                    // TCP shutdown is visible before the client has unwound Ports.
                    cleaned_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                    let rebound = UdpSocket::bind(("127.0.0.1", port)).unwrap();
                    drop(rebound);
                });
                session
                    .start_setup_selected(|| Ok(wire), Some(&credential()), selected)
                    .unwrap();
                let started = std::time::Instant::now();
                let result = if playback {
                    session.negotiate_realtime()
                } else {
                    session.check_initial_setup_with_timeout(
                        (fault == "cancel").then_some(Duration::from_secs(15)),
                    )
                };
                assert!(result.is_err());
                let error = format!("{:#}", result.unwrap_err());
                if fault == "cancel" {
                    assert!(error.contains("interrupted"), "{error}");
                    assert!(started.elapsed() < Duration::from_secs(2));
                } else if fault == "timeout" {
                    assert!(error.contains("timeout"), "{error}");
                    assert!(started.elapsed() < Duration::from_secs(5));
                }
                assert_eq!(session.state(), State::Failed);
                assert_eq!(session.packets_sent, 0);
                session.stop().unwrap();
                cleaned_tx.send(()).unwrap();
                server.join().unwrap();
            }
        }
    });
}

#[test]
fn preverify_info_cannot_bypass_pinned_identity_verification() {
    let (wire, server) = local_mock(|mut socket| {
        preverify_info_mock(&mut socket);
        assert!(verify_mock_core_offset(socket, Fault::Signature, PI, |_| {}, 1).is_none());
    });
    let mut session = Session::new();
    assert!(session
        .start_setup_selected(|| Ok(wire), Some(&credential()), SetupProfile::C)
        .is_err());
    assert_eq!(session.state(), State::Failed);
    assert_eq!(session.packets_sent, 0);
    server.join().unwrap();
}
