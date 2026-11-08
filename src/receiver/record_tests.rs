//! RECORD diagnostic tests: real SRP, independent cipher counters, loopback only.
use super::*;
use crate::receiver::session::RECORD_RECEIVER_ID;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    Good,
    StreamReject,
    StreamMalformed,
    StreamWrongCseq,
    VolumeReject,
    SyncTimeout,
    CancelStream,
    CancelMedia,
    RevokeMedia,
    MediaFeedback,
    RevokeStream,
    NoTiming,
    NoLengthOrLatency,
    Ptp,
    Reject,
    Timeout,
    Eof,
    WrongCseq,
    MissingCseq,
    Http,
    DuplicateLength,
    TransferEncoding,
    BadLength,
    BadTag,
    ExtraFrameData,
    Body,
    BodyTeardownReject,
    CancelAfterAuth,
    BadLatency,
    BadSession,
    TeardownReject,
    TeardownTimeout,
    TeardownEof,
    EventRefused,
    EventBadTag,
    CancelSetup,
    CancelRecord,
    CancelTeardown,
    RevokeSetup,
    RevokeRecord,
    RevokeTeardown,
}
impl Case {
    fn recorded(self) -> bool {
        matches!(
            self,
            Self::Good
                | Self::StreamReject
                | Self::StreamMalformed
                | Self::StreamWrongCseq
                | Self::VolumeReject
                | Self::SyncTimeout
                | Self::CancelMedia
                | Self::RevokeMedia
                | Self::MediaFeedback
                | Self::CancelStream
                | Self::RevokeStream
                | Self::NoLengthOrLatency
                | Self::NoTiming
                | Self::TeardownReject
                | Self::TeardownTimeout
                | Self::TeardownEof
                | Self::CancelTeardown
                | Self::RevokeTeardown
        )
    }
    fn success(self) -> bool {
        matches!(self, Self::Good | Self::NoTiming | Self::NoLengthOrLatency)
    }
}
fn state_files(root: &std::path::Path) -> BTreeMap<std::ffi::OsString, Vec<u8>> {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect()
}
fn record_case(case: Case) {
    record_case_mode(case, None);
}
fn record_case_mode(case: Case, playback: Option<bool>) {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!("receiver_allowlist=['{RECORD_RECEIVER_ID}']"),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let advertisement = mdns_sd::ServiceInfo::new(
        "_airplay._tcp.local.",
        "RecordFixture",
        "record.local.",
        "127.0.0.1",
        listener.local_addr().unwrap().port(),
        [
            ("deviceid", "3A:E5:C9:A3:02:AD"),
            ("pi", PI),
            ("features", "0x80200,0x10000"),
            ("flags", "0x98404"),
        ]
        .as_slice(),
    )
    .unwrap();
    let observed = vec![Receiver {
        id: RECORD_RECEIVER_ID.into(),
        name: "RecordFixture".into(),
        services: vec![discovery::Advertisement::test_from_info(&advertisement)],
    }];
    let selected = Selection::from_receiver(&observed[0], Method::Transient).unwrap();
    let store = store::Store::new(&config).unwrap();
    // Existing credentials for another receiver must survive byte-for-byte.
    store.save(&credential()).unwrap();
    store.select(&selected).unwrap();
    store.client_identity().unwrap();
    let identity = dir.path().join("receiver-state/client-identity.toml");
    std::fs::write(
        &identity,
        "client = \"123456789ABCDEF0\"\nremote = 123456789\n",
    )
    .unwrap();
    std::fs::set_permissions(&identity, std::fs::Permissions::from_mode(0o600)).unwrap();
    let before = state_files(&dir.path().join("receiver-state"));
    let revoke = config.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let remote_cancel = cancel.clone();
    let timing_addr = Arc::new(std::sync::Mutex::new(None));
    let remote_timing = timing_addr.clone();
    let local_media = Arc::new(std::sync::Mutex::new(Vec::new()));
    let remote_media = local_media.clone();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        owntone_info(&mut socket, 200, &info_body(0));
        setup_mock_transport(
            socket,
            Fault::None,
            Method::Transient,
            "3939",
            PI,
            false,
            true,
            move |mut socket, shared| {
                socket
                    .set_read_timeout(Some(Duration::from_secs(6)))
                    .unwrap();
                if case == Case::CancelAfterAuth {
                    assert_eof(&mut socket);
                    return;
                }
                let mut keys = ControlKeys::new(shared.as_ref());
                keys.swap_directions_for_test();
                let info = read_message(&mut socket, &mut keys);
                assert_eq!(
                    (&*info.method, &*info.uri, &*info.headers["cseq"]),
                    ("GET", "/info", "4")
                );
                send_reply(
                    &mut socket,
                    &mut keys,
                    &info,
                    Some(profile::dict([(
                        "features",
                        profile::uint((1 << 9) | (1 << 19)),
                    )])),
                    "",
                );
                let setup = read_message(&mut socket, &mut keys);
                assert_eq!((&*setup.method, &*setup.headers["cseq"]), ("SETUP", "5"));
                let value = plist(&setup);
                let offer = value.as_dictionary().unwrap();
                assert_eq!(offer.len(), 4);
                assert_eq!(offer["timingProtocol"].as_string(), Some("NTP"));
                assert_eq!(
                    offer["deviceID"].as_string(),
                    Some("12:34:56:78:9A:BC:DE:F0")
                );
                let timing = profile::port(offer, "timingPort").unwrap();
                *remote_timing.lock().unwrap() = Some(timing);
                if !matches!(case, Case::NoTiming | Case::SyncTimeout) {
                    timing_probe(timing);
                }
                let events = TcpListener::bind("127.0.0.1:0").unwrap();
                let event_port = events.local_addr().unwrap().port();
                if case == Case::CancelSetup {
                    remote_cancel.store(true, Ordering::Relaxed);
                }
                if case == Case::RevokeSetup {
                    std::fs::write(&revoke, "receiver_allowlist=[]").unwrap();
                }
                // Refusal uses a closed listener; other cases retain it until accept.
                let events = if case == Case::EventRefused {
                    drop(events);
                    None
                } else {
                    Some(events)
                };
                if case == Case::CancelSetup {
                    assert_eof(&mut socket);
                    return;
                }
                if case == Case::Ptp {
                    send_reply(
                        &mut socket,
                        &mut keys,
                        &setup,
                        Some(profile::dict([
                            ("timingProtocol", Value::String("PTP".into())),
                            ("eventPort", profile::uint(u64::from(event_port))),
                        ])),
                        "",
                    );
                    let end = read_message(&mut socket, &mut keys);
                    assert_eq!(end.method, "TEARDOWN");
                    assert_eq!(end.headers["cseq"], "6");
                    timing_probe(timing);
                    send_reply(&mut socket, &mut keys, &end, None, "");
                    assert_eof(&mut socket);
                    return;
                }
                send_reply(
                    &mut socket,
                    &mut keys,
                    &setup,
                    Some(profile::dict([(
                        "eventPort",
                        profile::uint(u64::from(event_port)),
                    )])),
                    "",
                );
                if case == Case::RevokeSetup {
                    assert_eof(&mut socket);
                    return;
                }
                let mut event_socket = None;
                if !matches!(case, Case::CancelSetup | Case::EventRefused) {
                    let events = events.unwrap();
                    events.set_nonblocking(true).unwrap();
                    let deadline = std::time::Instant::now() + Duration::from_secs(3);
                    let (mut event, _) = loop {
                        match events.accept() {
                            Ok(pair) => break pair,
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                assert!(std::time::Instant::now() < deadline);
                                thread::park_timeout(Duration::from_millis(1));
                            }
                            Err(e) => panic!("{e}"),
                        }
                    };
                    event
                        .set_read_timeout(Some(Duration::from_secs(4)))
                        .unwrap();
                    if case == Case::EventBadTag {
                        event
                            .write_all(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
                            .unwrap();
                        assert_eof(&mut event);
                    } else {
                        // Prove event-key derivation and encrypted ACK, not just TCP accept.
                        let mut event_keys = ControlKeys::events(shared.as_ref());
                        event_keys.swap_directions_for_test();
                        let mut body = Vec::new();
                        profile::dict([
                            ("type", Value::String("updateInfo".into())),
                            ("value", profile::dict([])),
                        ])
                        .to_writer_binary(&mut body)
                        .unwrap();
                        let mut request = format!("POST /command RTSP/1.0\r\nCSeq: 19\r\nContent-Type: application/x-apple-binary-plist\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
                        request.extend(body);
                        event
                            .write_all(&event_keys.encrypt(&request).unwrap())
                            .unwrap();
                        assert!(read_frame(&mut event, &mut event_keys)
                            .starts_with(b"RTSP/1.0 200 OK\r\nCSeq: 19\r\n"));
                    }
                    event_socket = Some(event);
                }
                let next = read_message(&mut socket, &mut keys);
                let teardown = if matches!(case, Case::CancelSetup | Case::EventRefused)
                    || next.method == "TEARDOWN"
                {
                    assert!(matches!(
                        case,
                        Case::CancelSetup | Case::EventRefused | Case::EventBadTag
                    ));
                    assert_eq!(next.method, "TEARDOWN");
                    assert_eq!(next.headers["cseq"], "6");
                    next
                } else {
                    assert_eq!(next.method, "RECORD");
                    assert_eq!(next.uri, setup.uri);
                    assert_eq!(next.headers["cseq"], "6");
                    assert!(next.body.is_empty());
                    assert_eq!(next.headers["content-length"], "0");
                    assert_eq!(next.headers.len(), 9); // common envelope + Session only
                    assert!(!next.headers.contains_key("content-type"));
                    assert!(!next.headers.contains_key("rtp-info"));
                    assert!(!next.headers.contains_key("range"));
                    for key in [
                        "session",
                        "user-agent",
                        "client-instance",
                        "dacp-id",
                        "active-remote",
                        "x-apple-client-name",
                        "x-apple-protocolversion",
                    ] {
                        assert_eq!(next.headers[key], setup.headers[key]);
                    }
                    if !matches!(case, Case::NoTiming | Case::SyncTimeout) {
                        timing_probe(timing);
                    }
                    if case == Case::CancelRecord {
                        remote_cancel.store(true, Ordering::Relaxed);
                    }
                    if case == Case::RevokeRecord {
                        std::fs::write(&revoke, "receiver_allowlist=[]").unwrap();
                    }
                    let malformed = match case {
                        Case::Reject => Some("RTSP/1.0 403 private-reason\r\nCSeq: 6\r\nContent-Length: 0\r\n\r\n"),
                        Case::WrongCseq => Some("RTSP/1.0 200 OK\r\nCSeq: 5\r\nContent-Length: 0\r\n\r\n"),
                        Case::MissingCseq => Some("RTSP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n"),
                        Case::Http => Some("HTTP/1.1 200 OK\r\nCSeq: 6\r\nContent-Length: 0\r\n\r\n"),
                        Case::DuplicateLength => Some("RTSP/1.0 200 OK\r\nCSeq: 6\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n"),
                        Case::TransferEncoding => Some("RTSP/1.0 200 OK\r\nCSeq: 6\r\nTransfer-Encoding: chunked\r\n\r\n"),
                        Case::BadLength => Some("RTSP/1.0 200 OK\r\nCSeq: 6\r\nContent-Length: -1\r\n\r\n"),
                        Case::ExtraFrameData => Some("RTSP/1.0 200 OK\r\nCSeq: 6\r\nContent-Length: 0\r\n\r\nunsolicited"),
                        _ => None,
                    };
                    if matches!(
                        case,
                        Case::Timeout | Case::CancelRecord | Case::RevokeRecord
                    ) {
                        assert_eof(&mut socket);
                        if let Some(mut event) = event_socket {
                            assert_eof(&mut event);
                        }
                        return;
                    }
                    if case == Case::Eof {
                        return;
                    }
                    if let Some(raw) = malformed {
                        socket
                            .write_all(&keys.encrypt(raw.as_bytes()).unwrap())
                            .unwrap();
                        assert_eof(&mut socket);
                        if let Some(mut event) = event_socket {
                            assert_eof(&mut event);
                        }
                        return;
                    }
                    if case == Case::BadTag {
                        let mut frame =
                            keys.encrypt(b"RTSP/1.0 200 OK\r\nCSeq: 6\r\n\r\n").unwrap();
                        *frame.last_mut().unwrap() ^= 1;
                        socket.write_all(&frame).unwrap();
                        assert_eof(&mut socket);
                        if let Some(mut event) = event_socket {
                            assert_eof(&mut event);
                        }
                        return;
                    }
                    let extra = match case {
                        Case::BadLatency => "Audio-Latency: -1\r\n",
                        Case::BadSession => "", // custom response below avoids duplicate Session
                        _ => "Audio-Latency: 88200\r\n",
                    };
                    if case == Case::NoLengthOrLatency {
                        socket.write_all(&keys.encrypt(b"RTSP/1.0 200 OK\r\nCSeq: 6\r\nSession: test-session\r\n\r\n").unwrap()).unwrap();
                    } else if case == Case::BadSession {
                        socket.write_all(&keys.encrypt(b"RTSP/1.0 200 OK\r\nCSeq: 6\r\nSession: changed\r\nContent-Length: 0\r\n\r\n").unwrap()).unwrap();
                    } else {
                        send_reply(
                            &mut socket,
                            &mut keys,
                            &next,
                            matches!(case, Case::Body | Case::BodyTeardownReject)
                                .then(|| profile::dict([])),
                            extra,
                        );
                    }
                    let mut next_seq = 7;
                    if let Some(playback) = playback.filter(|_| case.recorded()) {
                        let stream = read_message(&mut socket, &mut keys);
                        assert_eq!(stream.method, "SETUP");
                        assert_eq!(stream.uri, setup.uri);
                        assert_eq!(stream.headers["cseq"], "7");
                        assert_eq!(stream.headers["user-agent"], "AirPlay/999.0.0");
                        let value = plist(&stream);
                        let fields = value.as_dictionary().unwrap()["streams"]
                            .as_array()
                            .unwrap()[0]
                            .as_dictionary()
                            .unwrap();
                        assert_eq!(fields.len(), 13);
                        assert_eq!(fields["shk"].as_data().unwrap(), &shared[..32]);
                        assert_eq!(fields["type"].as_unsigned_integer(), Some(96));
                        let sid = stream
                            .uri
                            .rsplit('/')
                            .next()
                            .unwrap()
                            .parse::<u64>()
                            .unwrap();
                        assert_eq!(
                            fields["streamConnectionID"].as_unsigned_integer(),
                            Some(sid)
                        );
                        remote_media
                            .lock()
                            .unwrap()
                            .push(std::net::SocketAddr::from((
                                [127, 0, 0, 1],
                                profile::port(fields, "controlPort").unwrap(),
                            )));
                        let expected = profile::stream_offer(
                            shared[..32].try_into().unwrap(),
                            profile::port(fields, "controlPort").unwrap(),
                            sid as u32,
                        )
                        .unwrap();
                        assert_eq!(
                            value,
                            Value::from_reader(std::io::Cursor::new(&*expected)).unwrap()
                        );
                        let data = UdpSocket::bind("127.0.0.1:0").unwrap();
                        let control = UdpSocket::bind("127.0.0.1:0").unwrap();
                        data.set_read_timeout(Some(Duration::from_millis(500)))
                            .unwrap();
                        control
                            .set_read_timeout(Some(Duration::from_millis(500)))
                            .unwrap();
                        if matches!(
                            case,
                            Case::StreamReject
                                | Case::StreamWrongCseq
                                | Case::CancelStream
                                | Case::RevokeStream
                        ) {
                            if case == Case::CancelStream {
                                remote_cancel.store(true, Ordering::Relaxed);
                            } else if case == Case::RevokeStream {
                                std::fs::write(&revoke, "receiver_allowlist=[]").unwrap();
                            } else {
                                let response = if case == Case::StreamReject {
                                    b"RTSP/1.0 500 private-reason\r\nCSeq: 7\r\nContent-Length: 0\r\n\r\n".as_slice()
                                } else {
                                    b"RTSP/1.0 200 OK\r\nCSeq: 6\r\nContent-Length: 0\r\n\r\n"
                                        .as_slice()
                                };
                                socket.write_all(&keys.encrypt(response).unwrap()).unwrap();
                            }
                            assert_eof(&mut socket);
                            if let Some(mut event) = event_socket {
                                assert_eof(&mut event);
                            }
                            return;
                        }
                        if case == Case::StreamMalformed {
                            send_reply(
                                &mut socket,
                                &mut keys,
                                &stream,
                                Some(profile::dict([])),
                                "",
                            );
                            let end = read_message(&mut socket, &mut keys);
                            assert_eq!(end.method, "TEARDOWN");
                            assert_eq!(end.headers["cseq"], "8");
                            send_reply(&mut socket, &mut keys, &end, None, "");
                            assert_eof(&mut socket);
                            if let Some(mut event) = event_socket {
                                assert_eof(&mut event);
                            }
                            return;
                        }
                        send_reply(
                            &mut socket,
                            &mut keys,
                            &stream,
                            Some(profile::dict([(
                                "streams",
                                Value::Array(vec![profile::dict([
                                    (
                                        "dataPort",
                                        profile::uint(u64::from(data.local_addr().unwrap().port())),
                                    ),
                                    (
                                        "controlPort",
                                        profile::uint(u64::from(
                                            control.local_addr().unwrap().port(),
                                        )),
                                    ),
                                ])]),
                            )])),
                            "",
                        );
                        next_seq += 1;
                        if playback {
                            let volume = read_message(&mut socket, &mut keys);
                            assert_eq!(volume.method, "SET_PARAMETER");
                            assert_eq!(volume.headers["cseq"], "8");
                            assert_eq!(volume.headers["content-type"], "text/parameters");
                            assert_eq!(volume.body, b"volume: -40.000000\r\n");
                            if case == Case::VolumeReject {
                                socket.write_all(&keys.encrypt(b"RTSP/1.0 403 private-reason\r\nCSeq: 8\r\nContent-Length: 0\r\n\r\n").unwrap()).unwrap();
                                assert_eof(&mut socket);
                                if let Some(mut event) = event_socket {
                                    assert_eof(&mut event);
                                }
                                return;
                            }
                            send_reply(&mut socket, &mut keys, &volume, None, "");
                            if case == Case::SyncTimeout {
                                let end = read_message(&mut socket, &mut keys);
                                assert_eq!(end.method, "TEARDOWN");
                                assert_eq!(end.headers["cseq"], "9");
                                let mut b = [0; 2048];
                                assert!(data.recv_from(&mut b).is_err());
                                send_reply(&mut socket, &mut keys, &end, None, "");
                                assert_eof(&mut socket);
                                if let Some(mut event) = event_socket {
                                    assert_eof(&mut event);
                                }
                                return;
                            }

                            next_seq += 1;
                            let began = std::time::Instant::now();
                            let mut first_seq = None;
                            let mut first_ts = 0u32;
                            let mut first_nonce = 0u64;
                            let mut count = 0u64;
                            let expected_packets = 132300_u64.div_ceil(352);
                            let mut b = [0; 2048];
                            while count < expected_packets {
                                let (n, sender) = data.recv_from(&mut b).unwrap();
                                if count == 0 {
                                    remote_media.lock().unwrap().push(sender);
                                }
                                if count == 0
                                    && matches!(
                                        case,
                                        Case::CancelMedia | Case::RevokeMedia | Case::MediaFeedback
                                    )
                                {
                                    match case {
                                        Case::CancelMedia => {
                                            remote_cancel.store(true, Ordering::Relaxed)
                                        }
                                        Case::RevokeMedia => {
                                            std::fs::write(&revoke, "receiver_allowlist=[]")
                                                .unwrap()
                                        }
                                        Case::MediaFeedback => {
                                            control
                                                .send_to(
                                                    &[0x80, 0xd5, 0, 1, 0, 0, 0, 1],
                                                    (
                                                        "127.0.0.1",
                                                        profile::port(fields, "controlPort")
                                                            .unwrap(),
                                                    ),
                                                )
                                                .unwrap();
                                        }
                                        _ => unreachable!(),
                                    }
                                    if case == Case::RevokeMedia {
                                        assert_eof(&mut socket);
                                    } else {
                                        let end = read_message(&mut socket, &mut keys);
                                        assert_eq!(end.method, "TEARDOWN");
                                        assert_eq!(end.headers["cseq"], "9");
                                        send_reply(&mut socket, &mut keys, &end, None, "");
                                        assert_eof(&mut socket);
                                    }
                                    if let Some(mut event) = event_socket {
                                        assert_eof(&mut event);
                                    }
                                    return;
                                }
                                let p = &b[..n];
                                assert_eq!(p[0], 0x80);
                                assert_eq!(p[1], if count == 0 { 0xe0 } else { 0x60 });
                                let seq = u16::from_be_bytes(p[2..4].try_into().unwrap());
                                let ts = u32::from_be_bytes(p[4..8].try_into().unwrap());
                                let nonce = u64::from_le_bytes(p[n - 8..].try_into().unwrap());
                                if first_seq.is_none() {
                                    first_seq = Some(seq);
                                    first_ts = ts;
                                    first_nonce = nonce;
                                }
                                assert_eq!(seq, first_seq.unwrap().wrapping_add(count as u16));
                                assert_eq!(ts, first_ts.wrapping_add(count as u32 * 352));
                                assert_eq!(nonce, first_nonce + count);
                                assert_eq!(nonce, u64::from(seq));
                                // Receiver validates directly with the primitive, not sender helpers.
                                use chacha20poly1305::{
                                    aead::{Aead, KeyInit, Payload},
                                    ChaCha20Poly1305,
                                };
                                let mut iv = [0; 12];
                                iv[4..].copy_from_slice(&p[n - 8..]);
                                let codec = ChaCha20Poly1305::new_from_slice(&shared[..32])
                                    .unwrap()
                                    .decrypt(
                                        (&iv).into(),
                                        Payload {
                                            msg: &p[12..n - 8],
                                            aad: &p[4..12],
                                        },
                                    )
                                    .unwrap();
                                let pcm = crate::audio::alac::tests::decode_at_rate(&codec, 44100);
                                assert_eq!(pcm.len(), 352);
                                assert!(pcm.iter().flatten().all(|x| x.abs() <= 4100));
                                if count * 352 >= 44100 {
                                    assert!(pcm.iter().flatten().all(|x| *x == 0));
                                }
                                count += 1;
                            }
                            assert!(began.elapsed() >= Duration::from_millis(2800));
                            assert!(began.elapsed() < Duration::from_secs(4));
                            let (n, _) = control.recv_from(&mut b).unwrap();
                            assert_eq!(n, 20);
                            assert_eq!(&b[..4], &[0x90, 0xd4, 0, 7]);
                            assert_eq!(u32::from_be_bytes(b[16..20].try_into().unwrap()), first_ts);
                        } else {
                            let mut b = [0; 2048];
                            assert!(data.recv_from(&mut b).is_err());
                            assert!(control.recv_from(&mut b).is_err());
                        }
                    }
                    let end = read_message(&mut socket, &mut keys);
                    assert_eq!(end.headers["cseq"], next_seq.to_string());
                    end
                };
                assert_eq!(teardown.method, "TEARDOWN");
                assert_eq!(teardown.uri, setup.uri);
                assert!(plist(&teardown).as_dictionary().unwrap().is_empty());
                assert_eq!(teardown.headers["session"], "test-session");
                if !matches!(case, Case::NoTiming | Case::SyncTimeout) {
                    timing_probe(timing);
                }
                if case == Case::CancelTeardown {
                    remote_cancel.store(true, Ordering::Relaxed);
                }
                if case == Case::RevokeTeardown {
                    std::fs::write(&revoke, "receiver_allowlist=[]").unwrap();
                    assert_eof(&mut socket);
                } else if case == Case::TeardownTimeout {
                    assert_eof(&mut socket);
                } else if case == Case::TeardownEof {
                    return;
                } else if matches!(case, Case::TeardownReject | Case::BodyTeardownReject) {
                    socket.write_all(&keys.encrypt(format!("RTSP/1.0 500 private-reason\r\nCSeq: {}\r\nContent-Length: 0\r\n\r\n", teardown.headers["cseq"]).as_bytes()).unwrap()).unwrap();
                    assert_eof(&mut socket);
                } else {
                    send_reply(&mut socket, &mut keys, &teardown, None, "");
                    assert_eof(&mut socket);
                }
                if let Some(mut event) = event_socket {
                    assert_eof(&mut event);
                }
            },
        );
    });
    let mut session = Session::new();
    session.cancel = cancel;
    if let Some(playback) = playback {
        session
            .start_owntone_stream_selected(
                || Wire::connect(&config, RECORD_RECEIVER_ID, &selected, &observed),
                &selected.capabilities,
                playback,
            )
            .unwrap();
    } else {
        session
            .start_owntone_record_selected(
                || Wire::connect(&config, RECORD_RECEIVER_ID, &selected, &observed),
                &selected.capabilities,
            )
            .unwrap();
    }
    assert!(session.negotiate_realtime_at_volume(-30).is_err());
    if case == Case::CancelAfterAuth {
        session.cancel.store(true, Ordering::Relaxed);
    }
    let output = diagnostics::tests::capture(tracing::Level::DEBUG, || {
        let result = match playback {
            None => session.check_record(),
            Some(false) => session.check_stream(),
            Some(true) => session
                .prepare_playback_check()
                .and_then(|()| session.transmit_test(1))
                .and_then(|()| session.stop()),
        };
        assert_eq!(result.is_ok(), case.success(), "{case:?}: {result:?}");
        if let Err(error) = result {
            let error = format!("{error:#}");
            assert!(!error.contains("private-reason"));
            if case == Case::BodyTeardownReject {
                assert!(error.contains("unexpected RECORD response body"));
                assert!(error.contains("bounded TEARDOWN also failed"));
            }
        }
    });
    assert_eq!(
        session.record_succeeded(),
        case.recorded(),
        "{case:?}: {output}"
    );
    assert_eq!(
        session.state(),
        if case.success() {
            State::Disconnected
        } else {
            State::Failed
        }
    );
    if matches!(
        case,
        Case::CancelMedia | Case::RevokeMedia | Case::MediaFeedback
    ) {
        assert!((1..=10).contains(&session.packets_sent));
    } else {
        assert_eq!(
            session.packets_sent,
            if playback == Some(true) && (case.success() || case == Case::TeardownReject) {
                376
            } else {
                0
            }
        );
    }
    assert_eq!(session.resends, 0);
    assert!(!output.contains("private-reason"));
    if case.recorded() {
        assert!(output.contains("RECORD validated"));
    }
    if matches!(
        case,
        Case::TeardownReject | Case::TeardownTimeout | Case::TeardownEof | Case::RevokeTeardown
    ) {
        assert!(!output.contains("\"outcome\":\"acknowledged\""));
    }
    session.stop().unwrap();
    server.join().unwrap();
    assert_eq!(state_files(&dir.path().join("receiver-state")), before);
    for addr in local_media.lock().unwrap().iter() {
        UdpSocket::bind(addr).unwrap();
    }
    if let Some(port) = *timing_addr.lock().unwrap() {
        UdpSocket::bind(("127.0.0.1", port)).unwrap();
    };
}
#[test]
fn record_contract_same_connection_counters_events_timing_and_cleanup() {
    for case in [
        Case::Good,
        Case::NoTiming,
        Case::NoLengthOrLatency,
        Case::Ptp,
        Case::Reject,
        Case::Timeout,
        Case::Eof,
        Case::WrongCseq,
        Case::MissingCseq,
        Case::Http,
        Case::DuplicateLength,
        Case::TransferEncoding,
        Case::BadLength,
        Case::BadTag,
        Case::ExtraFrameData,
        Case::Body,
        Case::BodyTeardownReject,
        Case::CancelAfterAuth,
        Case::BadLatency,
        Case::BadSession,
        Case::TeardownReject,
        Case::TeardownTimeout,
        Case::TeardownEof,
        Case::EventRefused,
        Case::EventBadTag,
        Case::CancelSetup,
        Case::CancelRecord,
        Case::CancelTeardown,
        Case::RevokeSetup,
        Case::RevokeRecord,
        Case::RevokeTeardown,
    ] {
        record_case(case);
    }
}

#[test]
fn record_rejects_unsupported_policy_and_precancel_before_connect() {
    let mut caps = owntone_caps();
    caps.password = Some(true);
    assert!(Session::new()
        .start_owntone_record_selected(|| panic!("must not connect"), &caps)
        .is_err());
    let mut session = Session::new();
    session.cancel.store(true, Ordering::Relaxed);
    assert!(session
        .start_owntone_record_selected(|| panic!("cancelled before connect"), &owntone_caps())
        .is_err());
    assert!(!session.record_succeeded());
    assert_eq!(session.packets_sent, 0);
    // The lower-level gate refuses a mismatched ID before authentication bytes.
    let (wire, server) = local_mock_method(Method::Transient, |mut socket| assert_eof(&mut socket));
    assert!(Session::new()
        .start_owntone_record_selected(|| Ok(wire), &owntone_caps())
        .is_err());
    server.join().unwrap();
}

#[test]
fn stream_setup_only_uses_verified_key_and_sends_zero_media_or_volume() {
    record_case_mode(Case::Good, Some(false));
}
#[test]
fn playback_check_receiver_decrypts_decodes_and_validates_bounded_pacing() {
    record_case_mode(Case::Good, Some(true));
}

#[test]
fn stream_and_playback_fail_closed_on_stream_transport_schema_cancellation_and_revocation() {
    for playback in [false, true] {
        for case in [
            Case::StreamReject,
            Case::StreamMalformed,
            Case::StreamWrongCseq,
            Case::CancelStream,
            Case::RevokeStream,
        ] {
            record_case_mode(case, Some(playback));
        }
    }
    for case in [Case::VolumeReject, Case::SyncTimeout] {
        record_case_mode(case, Some(true));
    }
}

#[test]
fn playback_stops_promptly_on_media_cancellation_revocation_and_resend_request() {
    for case in [Case::CancelMedia, Case::RevokeMedia, Case::MediaFeedback] {
        record_case_mode(case, Some(true));
    }
}

#[test]
fn both_stream_modes_report_failed_remote_teardown_and_release_locally() {
    record_case_mode(Case::TeardownReject, Some(false));
    record_case_mode(Case::TeardownReject, Some(true));
}

#[test]
fn new_modes_preserve_event_health_early_revocation_and_precontact_capability_gates() {
    for playback in [false, true] {
        for case in [
            Case::EventBadTag,
            Case::EventRefused,
            Case::CancelSetup,
            Case::CancelRecord,
            Case::RevokeSetup,
            Case::RevokeRecord,
        ] {
            record_case_mode(case, Some(playback));
        }
        let mut caps = owntone_caps();
        caps.password = Some(true);
        assert!(Session::new()
            .start_owntone_stream_selected(
                || panic!("unsupported policy must not connect"),
                &caps,
                playback
            )
            .is_err());
        let mut session = Session::new();
        session.cancel.store(true, Ordering::Relaxed);
        assert!(session
            .start_owntone_stream_selected(
                || panic!("cancelled before contact"),
                &owntone_caps(),
                playback
            )
            .is_err());
    }
    // Stream-only negotiation reports timing count without demanding a probe.
    record_case_mode(Case::NoTiming, Some(false));
}
