use super::control::{acknowledgement, AnchorRequest, Peers, Rate};
use super::*;
use crate::receiver::profile;
use std::net::{Ipv4Addr, SocketAddr};
use time::{SampleTimeline, Timestamp};
fn ts(ns: i128) -> Timestamp {
    Timestamp::from_ns(ns).unwrap()
}
fn request(kind: Kind, seq: u16) -> Packet {
    Packet::new(
        kind,
        0x1122334455667788,
        seq,
        vec![0; if kind == Kind::PdelayReq { 20 } else { 10 }],
    )
}
fn peer(clock: &mut Grandmaster, now: Instant, seq: u16) {
    clock
        .receive(&request(Kind::DelayReq, seq).encode().unwrap(), now)
        .unwrap();
}
#[test]
fn ieee_known_answer_and_malformed() {
    // Hand assembled from IEEE 1588-2008 tables 18/26, not encoder output.
    let bytes = [
        0x11, 2, 0, 44, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x11, 0x22, 0x33, 0x44,
        0x55, 0x66, 0x77, 0x88, 0x80, 5, 0xff, 0xff, 1, 0, 0, 1, 2, 3, 4, 5, 0x3b, 0x9a, 0xc9,
        0xff,
    ];
    let p = Packet::decode(&bytes).unwrap();
    assert_eq!(
        p.timestamp().unwrap(),
        Timestamp::new(0x000102030405, 999_999_999).unwrap()
    );
    assert_eq!(p.sequence, 65535);
    assert_eq!(p.encode().unwrap(), bytes);
    for n in 0..bytes.len() {
        assert!(Packet::decode(&bytes[..n]).is_err());
    }
    for (index, val) in [
        (0, 0x21),
        (1, 1),
        (2, 1),
        (4, 1),
        (5, 1),
        (16, 1),
        (28, 0),
        (40, 255),
    ] {
        let mut b = bytes;
        b[index] = val;
        if index == 28 {
            b[29] = 0;
        }
        assert!(Packet::decode(&b).is_err(), "index {index}");
    }
    let mut b = bytes.to_vec();
    b.extend_from_slice(&[0, 3, 0, 8]);
    b[3] += 4;
    assert!(Packet::decode(&b).is_err());
}
#[test]
fn all_message_layouts_and_signed_correction() {
    for kind in [
        Kind::Sync,
        Kind::DelayReq,
        Kind::PdelayReq,
        Kind::PdelayResp,
        Kind::FollowUp,
        Kind::DelayResp,
        Kind::PdelayFollowUp,
        Kind::Announce,
        Kind::Signaling,
    ] {
        let n = match kind {
            Kind::Sync | Kind::DelayReq | Kind::FollowUp | Kind::Signaling => 10,
            Kind::Announce => 30,
            _ => 20,
        };
        let mut p = Packet::new(kind, 42, 7, vec![0; n]);
        p.correction = -65537;
        assert_eq!(Packet::decode(&p.encode().unwrap()).unwrap(), p);
    }
}
#[test]
fn precision_rollover_and_absolute_sample_mapping() {
    assert_eq!(
        Timestamp::new(7, 500_000_000).unwrap().network(),
        NetworkTime {
            seconds: 7,
            fraction: 1 << 63
        }
    );
    let t = Timestamp::new((1 << 48) - 1, 999_999_999).unwrap();
    assert_eq!(Timestamp::decode(&t.encode().unwrap()).unwrap(), t);
    assert!(Timestamp::new(1 << 48, 0).is_err());
    assert!(Timestamp::new(0, 1_000_000_000).is_err());
    let timeline = SampleTimeline::new(
        42,
        0,
        NetworkTime {
            seconds: 7,
            fraction: 1 << 63,
        },
        u64::from(u32::MAX) - 100,
    )
    .unwrap();
    let sample = u64::from(u32::MAX) - 100 + 48_000;
    assert_eq!(
        timeline.at(sample).unwrap(),
        NetworkTime {
            seconds: 8,
            fraction: 1 << 63
        }
    );
    assert_eq!(sample as u32, 47_899);
    assert_eq!(
        timeline.sample_at(timeline.at(sample).unwrap()).unwrap(),
        sample
    );
    // Direct rational conversion retains the fractional low word.
    let one = timeline.at(u64::from(u32::MAX) - 99).unwrap();
    assert_eq!(one.fraction - (1 << 63), ((1u128 << 64) / 48_000) as u64);
    assert_ne!(one.fraction & 0xffff_ffff, 0);
    assert!(timeline.at(u64::MAX).is_err());
    assert!(SampleTimeline::new(
        42,
        0,
        NetworkTime {
            seconds: 0,
            fraction: 0
        },
        100
    )
    .unwrap()
    .at(0)
    .is_err());
}
#[test]
fn offset_drift_lock_loss_discontinuity_recovery() {
    // Independent analytical exchange: 1ms path each way, local +2ms.
    let mut e = Estimator::new();
    for i in 0..4 {
        let master = 10_000_000_000 + i * 400_000_000;
        let drift = i * 40_000; // +100 ppm
        let m = Measurement::solve(
            [
                ts(master),
                ts(master + 3_000_000 + drift),
                ts(master + 4_000_000 + drift),
                ts(master + 3_000_000),
            ],
            [0, 0],
        )
        .unwrap();
        assert_eq!(m.delay_scaled, 1_000_000 * 65_536);
        assert_eq!(m.offset_scaled, (2_000_000 + drift) * 65_536);
        e.observe(m).unwrap();
        assert_eq!(
            e.state,
            if i == 3 {
                State::Locked
            } else {
                State::Synchronizing
            }
        );
    }
    assert!((e.drift_ppb - 100_000).abs() < 11);
    let last = e.last.unwrap();
    let synchronized = e.network_at(last.local_ns).unwrap();
    assert_eq!(
        synchronized.ns(),
        last.local_ns - last.offset_scaled / 65_536
    );
    assert!(e.network_at(last.local_ns + 3_000_000_001).is_err());
    assert_eq!(e.state, State::Lost);
    for i in 0..4 {
        e.observe(Measurement {
            local_ns: 20_000_000_000 + i * 400_000_000,
            offset_scaled: 0,
            delay_scaled: 0,
        })
        .unwrap();
    }
    assert_eq!(e.state, State::Locked);
    assert!(e
        .observe(Measurement {
            local_ns: 21_600_000_000,
            offset_scaled: 10_000_000 * 65_536,
            delay_scaled: 0
        })
        .is_err());
    assert_eq!(e.state, State::Lost);
    e.shutdown();
    assert!(e.observe(last).is_err());
    assert!(Measurement::solve([ts(3), ts(1), ts(0), ts(2)], [0, 0]).is_err());
}
#[test]
fn grandmaster_liveness_is_not_receiver_lock() {
    let now = Instant::now();
    let mut c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(2)).unwrap();
    assert_eq!(c.state(), State::Initializing);
    assert!(!c.media_permitted());
    let packets = c.due(now).unwrap();
    assert_eq!(packets.len(), 2);
    let sync = &packets[1];
    assert_eq!(sync.kind, Kind::Sync);
    assert_eq!(sync.flags, 0x0600);
    let fup = c.follow_up(sync, now + Duration::from_nanos(123)).unwrap();
    assert_eq!(fup.sequence, sync.sequence);
    assert_eq!(fup.timestamp().unwrap().nanos, 123);
    assert!(c.follow_up(sync, now + Duration::from_nanos(124)).is_err());
    assert_eq!(packets[0].body[14], 248); // never claims GPS quality
    peer(&mut c, now + Duration::from_millis(1), 65535);
    peer(&mut c, now + Duration::from_millis(2), 0); // sequence wrap
    assert_eq!(c.state(), State::PeerObserved);
    assert!(!c.media_permitted());
    assert!(c
        .receive(
            &request(Kind::DelayReq, 0).encode().unwrap(),
            now + Duration::from_millis(3)
        )
        .is_err());
    assert!(c
        .receive(
            &request(Kind::DelayReq, 65534).encode().unwrap(),
            now + Duration::from_millis(4)
        )
        .is_err());
    let mut changed = request(Kind::DelayReq, 1);
    changed.source[0] ^= 1;
    assert!(c
        .receive(&changed.encode().unwrap(), now + Duration::from_millis(5))
        .is_err());
    assert!(c.check(now + Duration::from_secs(4), false).is_err());
    assert_eq!(c.state(), State::Lost);
    assert!(c.network_time(now + Duration::from_secs(4)).is_err());
    // Fresh clock/session required, never silently rebase a timeline.
    assert!(c
        .receive(
            &request(Kind::DelayReq, 1).encode().unwrap(),
            now + Duration::from_secs(4)
        )
        .is_err());
}
#[test]
fn timeout_cancel_clock_reversal_and_pdelay() {
    let now = Instant::now();
    let mut c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(1)).unwrap();
    assert!(c.check(now + Duration::from_secs(1), false).is_err());
    let mut c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(1)).unwrap();
    assert!(c.check(now, true).is_err());
    assert_eq!(c.state(), State::Shutdown);
    let mut c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(1)).unwrap();
    assert!(c.check(now - Duration::from_nanos(1), false).is_err());
    let mut c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(1)).unwrap();
    let response = c
        .receive(&request(Kind::PdelayReq, 12).encode().unwrap(), now)
        .unwrap();
    assert_eq!(response[0].flags, 0x0600);
    assert_eq!(response[0].kind, Kind::PdelayResp);
    assert_eq!(response[0].body[10..], request(Kind::PdelayReq, 12).source);
}
#[test]
fn control_bplist_precision_acknowledgements_and_anchor_generation() {
    let peers = Peers::new("192.0.2.2".parse().unwrap(), "192.0.2.1".parse().unwrap()).unwrap();
    assert_eq!(
        profile::parse(&peers.encode().unwrap()).unwrap(),
        plist::Value::Array(vec!["192.0.2.2".into(), "192.0.2.1".into()])
    );
    assert!(Peers::new("0.0.0.0".parse().unwrap(), "192.0.2.1".parse().unwrap()).is_err());
    let now = Instant::now();
    let mut c = Grandmaster::new(
        0xffff112233445566,
        0x1122334455667788,
        now,
        Duration::from_secs(2),
    )
    .unwrap();
    let mapping = SampleTimeline::new(
        c.clock_id,
        c.generation(),
        NetworkTime {
            seconds: 1,
            fraction: (1 << 63) + 123,
        },
        0,
    )
    .unwrap();
    assert!(AnchorRequest::generate(&mut c, &mapping, 0, Rate::Play, now).is_err());
    peer(&mut c, now, 1);
    let a = AnchorRequest::generate(&mut c, &mapping, 0, Rate::Play, now).unwrap();
    let bytes = a.encode().unwrap();
    let v = profile::parse(&bytes).unwrap();
    let d = v.as_dictionary().unwrap();
    assert_eq!(
        d["networkTimeFrac"].as_signed_integer().unwrap() as u64,
        (1 << 63) + 123
    );
    assert_eq!(
        d["networkTimeTimelineID"].as_signed_integer().unwrap() as u64,
        c.clock_id
    );
    assert_eq!(d["rate"].as_unsigned_integer(), Some(1));
    // Object marker 0x13 + exact eight bytes, never a 16-byte unsigned integer.
    assert!(bytes
        .windows(9)
        .any(|b| b[0] == 0x13 && b[1..] == c.clock_id.to_be_bytes()));
    assert!(acknowledgement(&[], None).is_ok());
    let mut ack = profile::dict([("status", profile::uint(0))]);
    assert!(acknowledgement(&profile::binary(&mut ack).unwrap(), None).is_ok());
    let mut ack = profile::dict([("status", profile::uint(1))]);
    assert_eq!(
        acknowledgement(&profile::binary(&mut ack).unwrap(), None)
            .unwrap_err()
            .to_string(),
        "timing control rejected in-band: status=1"
    );
    let mut malformed = profile::dict([(
        "status",
        plist::Value::String("private receiver text".into()),
    )]);
    let error = acknowledgement(&profile::binary(&mut malformed).unwrap(), None).unwrap_err();
    assert_eq!(error.to_string(), "invalid timing control status");
    assert!(acknowledgement(b"garbage", None).is_err());
    assert!(acknowledgement(&bytes, Some(&v)).is_ok());
    let mut bad = v.clone();
    bad.as_dictionary_mut()
        .unwrap()
        .insert("rate".into(), profile::uint(2));
    assert!(acknowledgement(&profile::binary(&mut bad).unwrap(), Some(&v)).is_err());
    let stale = SampleTimeline::new(
        c.clock_id,
        c.generation() + 1,
        NetworkTime {
            seconds: 1,
            fraction: 0,
        },
        0,
    )
    .unwrap();
    assert!(AnchorRequest::generate(&mut c, &stale, 0, Rate::Pause, now).is_err());
}
fn udp_pair() -> (UdpSocket, UdpSocket) {
    let a = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let b = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    a.connect(b.local_addr().unwrap()).unwrap();
    b.connect(a.local_addr().unwrap()).unwrap();
    b.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    (a, b)
}
#[test]
fn loopback_service_and_socket_cleanup() {
    let (event, peer_event) = udp_pair();
    let (general, peer_general) = udp_pair();
    let event_addr = event.local_addr().unwrap();
    let general_addr = general.local_addr().unwrap();
    let now = Instant::now();
    let c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(2)).unwrap();
    let fixture = authorized_ptp_fixture();
    let mut s = Service::attach(c, event, general, fixture.wire.ptp_lease().unwrap()).unwrap();
    let cancel = AtomicBool::new(false);
    peer_event
        .send(&request(Kind::DelayReq, 1).encode().unwrap())
        .unwrap();
    s.poll(&cancel).unwrap();
    assert_eq!(s.state(), State::PeerObserved);
    let mut b = [0; 512];
    let n = peer_event.recv(&mut b).unwrap();
    assert_eq!(Packet::decode(&b[..n]).unwrap().kind, Kind::Sync);
    let mut kinds = vec![];
    for _ in 0..3 {
        let n = peer_general.recv(&mut b).unwrap();
        kinds.push(Packet::decode(&b[..n]).unwrap().kind);
    }
    assert_eq!(kinds, vec![Kind::Announce, Kind::FollowUp, Kind::DelayResp]);
    cancel.store(true, Ordering::Relaxed);
    assert!(s.poll(&cancel).is_err());
    assert_eq!(s.state(), State::Shutdown);
    // Cancellation closes sockets immediately, even while the service owner lives.
    let _e = UdpSocket::bind(event_addr).unwrap();
    let _g = UdpSocket::bind(general_addr).unwrap();
    drop(_e);
    drop(_g);
    drop(s);
    drop(peer_event);
    drop(peer_general);
    let _e = UdpSocket::bind(event_addr).unwrap();
    let _g = UdpSocket::bind(general_addr).unwrap();
}
#[test]
fn socket_peer_mismatch_rejected() {
    let fixture = authorized_ptp_fixture();
    let (e, _) = udp_pair();
    let g = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    g.connect(SocketAddr::from(([127, 0, 0, 2], 320))).unwrap();
    assert!(Service::attach(
        Grandmaster::new(
            42,
            0x1122334455667788,
            Instant::now(),
            Duration::from_secs(1)
        )
        .unwrap(),
        e,
        g,
        fixture.wire.ptp_lease().unwrap(),
    )
    .is_err());
}

use crate::receiver::{crypto::ControlKeys, wire::Wire};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
};
fn read_control(socket: &mut TcpStream, keys: &mut ControlKeys) -> (String, Vec<u8>) {
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
        if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let header = String::from_utf8(bytes[..i + 4].to_vec()).unwrap();
            let length = header
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            if bytes.len() == i + 4 + length {
                return (header, bytes[i + 4..].to_vec());
            }
        }
        assert!(bytes.len() < 8192);
    }
}
fn control_reply(
    socket: &mut TcpStream,
    keys: &mut ControlKeys,
    seq: u32,
    body: &[u8],
    status: u16,
) {
    let mut raw = format!(
        "RTSP/1.0 {status} response\r\nCSeq: {seq}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    for chunk in raw.chunks(23) {
        socket.write_all(&keys.encrypt(chunk).unwrap()).unwrap();
    }
}
fn mock_timing(server: impl FnOnce(TcpStream) + Send + 'static) -> (Wire, thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::new(127, 0, 0, 2), 0)).unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let t = thread::spawn(move || {
        let (s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        server(s);
    });
    let wire = Wire::mock(socket, Duration::from_millis(100), "device:112233445566");
    (wire, t)
}
#[test]
fn authenticated_timing_control_same_socket_and_failure_semantics() {
    // 0: accepted; 1: in-band rejection; 2: bad CSeq; 3: RTSP rejection;
    // 4: contradictory rate; 5: malformed plist; 6: timeout.
    for fault in 0..7 {
        let (wire, t) = mock_timing(move |mut socket| {
            let mut keys = ControlKeys::new(b"timing-test");
            keys.swap_directions_for_test();
            let (head, body) = read_control(&mut socket, &mut keys);
            assert!(head.starts_with("SETPEERS rtsp://127.0.0.2/session RTSP/1.0\r\n"));
            assert!(head.contains("Content-Type: /peer-list-changed\r\n"));
            assert_eq!(
                profile::parse(&body).unwrap(),
                plist::Value::Array(vec!["127.0.0.2".into(), "127.0.0.1".into()])
            );
            control_reply(&mut socket, &mut keys, 1, &[], 200);
            let (head, body) = read_control(&mut socket, &mut keys);
            assert!(head.starts_with("SETRATEANCHORTIME rtsp://127.0.0.2/session RTSP/1.0\r\n"));
            assert!(head.contains("Content-Type: application/x-apple-binary-plist\r\n"));
            let anchor = profile::parse(&body).unwrap();
            assert_eq!(
                anchor.as_dictionary().unwrap()["rate"].as_unsigned_integer(),
                Some(1)
            );
            let response = match fault {
                1 => profile::binary(&mut profile::dict([("status", profile::uint(1))]))
                    .unwrap()
                    .to_vec(),
                4 => profile::binary(&mut profile::dict([("rate", profile::uint(0))]))
                    .unwrap()
                    .to_vec(),
                5 => b"garbage".to_vec(),
                _ => Vec::new(),
            };
            if fault != 6 {
                control_reply(
                    &mut socket,
                    &mut keys,
                    if fault == 2 { 3 } else { 2 },
                    &response,
                    if fault == 3 { 500 } else { 200 },
                );
            }
            let mut b = [0; 1];
            assert_eq!(socket.read(&mut b).unwrap(), 0);
        });
        let mut wire = wire
            .secure(
                ControlKeys::new(b"timing-test"),
                ControlKeys::events(b"timing-test"),
            )
            .unwrap();
        let _approval = wire.ptp_test_authorization().unwrap();
        let now = Instant::now();
        let mut clock =
            Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(2)).unwrap();
        peer(&mut clock, now, 1);
        let timeline = SampleTimeline::new(
            42,
            clock.generation(),
            NetworkTime {
                seconds: 1,
                fraction: 1 << 63,
            },
            0,
        )
        .unwrap();
        let a = AnchorRequest::generate(&mut clock, &timeline, 0, Rate::Play, now).unwrap();
        {
            let mut control = wire
                .buffered_timing("rtsp://127.0.0.2/session".into(), &clock)
                .unwrap();
            control.set_peers().unwrap();
            let result = control.set_rate_anchor_time(&mut clock, a, Instant::now());
            assert_eq!(result.is_ok(), fault == 0, "fault {fault}");
            assert!(control.set_peers().is_err());
        }
        if fault != 0 {
            assert!(wire.is_invalidated());
        }
        drop(wire);
        t.join().unwrap();
    }
}
#[test]
fn unauthenticated_timing_is_rejected_before_io() {
    let clock = Grandmaster::new(
        42,
        0x1122334455667788,
        Instant::now(),
        Duration::from_secs(1),
    )
    .unwrap();
    let (mut wire, t) = mock_timing(|mut socket| {
        let mut b = [0; 1];
        assert_eq!(socket.read(&mut b).unwrap(), 0);
    });
    assert!(wire
        .buffered_timing("rtsp://127.0.0.2/session".into(), &clock)
        .is_err());
    assert!(wire
        .start_ptp(
            42,
            0x1122334455667788,
            crate::receiver::lifecycle::TimingPeer::select(
                &[plist::Value::String("127.0.0.1".into())],
                "127.0.0.1".parse().unwrap()
            )
            .unwrap(),
            Duration::from_secs(1)
        )
        .is_err());
    drop(wire);
    t.join().unwrap();
}
#[test]
fn every_timing_state_keeps_buffered_playback_disabled() {
    let now = Instant::now();
    let mut clock = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(1)).unwrap();
    for state in [
        State::Initializing,
        State::Synchronizing,
        State::PeerObserved,
        State::Locked,
        State::Lost,
        State::Shutdown,
    ] {
        clock.state = state;
        assert!(!clock.media_permitted());
        let mut session = crate::receiver::session::Session::new();
        assert!(session.start_buffered_audio().is_err());
    }
}

fn unhex(s: &str) -> Vec<u8> {
    s.trim()
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
        .collect()
}
#[test]
fn independent_python_plist_fixtures() {
    let peer_bytes = unhex(include_str!("fixtures/peers.hex"));
    let peers = Peers::new("192.0.2.2".parse().unwrap(), "192.0.2.1".parse().unwrap()).unwrap();
    assert_eq!(peers.encode().unwrap().as_slice(), peer_bytes);
    let anchor_bytes = unhex(include_str!("fixtures/anchor.hex"));
    let clock_id = (-26263587901610i64) as u64;
    let now = Instant::now();
    let mut clock =
        Grandmaster::new(clock_id, 0x1122334455667788, now, Duration::from_secs(2)).unwrap();
    peer(&mut clock, now, 1);
    let timeline = SampleTimeline::new(
        clock_id,
        clock.generation(),
        NetworkTime {
            seconds: 1,
            fraction: (1 << 63) + 123,
        },
        0,
    )
    .unwrap();
    let a = AnchorRequest::generate(&mut clock, &timeline, 0, Rate::Play, now).unwrap();
    // Same independent encoding byte for byte, including signed 8-byte values.
    assert_eq!(a.encode().unwrap().as_slice(), anchor_bytes);
}
#[test]
fn timeline_owner_recreation_and_expired_evidence() {
    let now = Instant::now();
    let mut c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(1)).unwrap();
    peer(&mut c, now, 1);
    let stale = SampleTimeline::new(
        42,
        c.generation(),
        NetworkTime {
            seconds: 1,
            fraction: 0,
        },
        0,
    )
    .unwrap();
    let mut replacement =
        Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(1)).unwrap();
    peer(&mut replacement, now, 1);
    assert_ne!(c.generation(), replacement.generation());
    assert!(AnchorRequest::generate(&mut replacement, &stale, 0, Rate::Play, now).is_err());
    assert!(c.network_time(now + Duration::from_secs(4)).is_err());
    assert!(c.network_time(now - Duration::from_nanos(1)).is_err());
    let mut e = Estimator::new();
    let m = Measurement::solve([ts(1000), ts(1010), ts(1020), ts(1030)], [1, -1]).unwrap();
    assert_eq!(m.offset_scaled, -1); // correction fractions not truncated to ns
    e.observe(m).unwrap();
    assert!(e.observe(m).is_err());
    assert_eq!(e.state, State::Lost);
}

#[test]
fn malformed_flood_cannot_create_or_extend_timing_readiness() {
    let now = Instant::now();
    let mut c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(1)).unwrap();
    let mut wrong = request(Kind::DelayReq, 1);
    wrong.source[0] ^= 1;
    assert!(c.receive(&wrong.encode().unwrap(), now).is_err());
    assert_eq!(c.state(), State::Synchronizing);
    let signaling = Packet::new(Kind::Signaling, 0x1122334455667788, 1, vec![0; 10]);
    let mut malformed = signaling.encode().unwrap();
    malformed.extend_from_slice(&[0, 3, 0, 2, 0, 0]);
    malformed[3] = malformed.len() as u8;
    assert!(c
        .receive(&malformed, now + Duration::from_millis(500))
        .is_err());
    assert!(c.check(now + Duration::from_secs(1), false).is_err());
    assert_eq!(c.state(), State::Lost);
    let mut c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(1)).unwrap();
    peer(&mut c, now, 1);
    for i in 1..4 {
        assert!(c
            .receive(
                &request(Kind::DelayReq, 1).encode().unwrap(),
                now + Duration::from_secs(i)
            )
            .is_err());
    }
    assert!(c.check(now + Duration::from_secs(4), false).is_err());
}
#[test]
fn rejected_setpeers_closes_control_without_anchor_or_retry() {
    let (wire, t) = mock_timing(|mut socket| {
        let mut keys = ControlKeys::new(b"timing-test");
        keys.swap_directions_for_test();
        let (head, _) = read_control(&mut socket, &mut keys);
        assert!(head.starts_with("SETPEERS "));
        let body = profile::binary(&mut profile::dict([("status", profile::uint(7))])).unwrap();
        control_reply(&mut socket, &mut keys, 1, &body, 200);
        let mut b = [0; 1];
        assert_eq!(socket.read(&mut b).unwrap(), 0);
    });
    let mut wire = wire
        .secure(
            ControlKeys::new(b"timing-test"),
            ControlKeys::events(b"timing-test"),
        )
        .unwrap();
    let _approval = wire.ptp_test_authorization().unwrap();
    let clock = Grandmaster::new(
        42,
        0x1122334455667788,
        Instant::now(),
        Duration::from_secs(1),
    )
    .unwrap();
    {
        let mut control = wire
            .buffered_timing("rtsp://127.0.0.2/session".into(), &clock)
            .unwrap();
        assert!(control.set_peers().is_err());
        assert!(control.set_peers().is_err());
    }
    assert!(wire.is_invalidated());
    drop(wire);
    t.join().unwrap();
}

/// Network fixtures opt in to real allowlist/selection checks. Mock encryption
/// keys alone never grant PTP authorization. Keep the Wire and config owner alive.
struct AuthorizedPtpFixture {
    wire: Wire,
    approval: tempfile::TempDir,
    _receiver: TcpStream,
}
fn authorized_ptp_fixture() -> AuthorizedPtpFixture {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (receiver, _) = listener.accept().unwrap();
    let mut wire = Wire::mock(socket, Duration::from_secs(1), "device:112233445566")
        .secure(
            ControlKeys::new(b"ptp-fixture"),
            ControlKeys::events(b"ptp-fixture"),
        )
        .unwrap();
    let approval = wire.ptp_test_authorization().unwrap();
    AuthorizedPtpFixture {
        wire,
        approval,
        _receiver: receiver,
    }
}
impl AuthorizedPtpFixture {
    fn revoke(&self) {
        std::fs::write(
            self.approval.path().join("config.toml"),
            "receiver_allowlist=[]",
        )
        .unwrap();
    }
}
fn fixture_clock() -> Grandmaster {
    Grandmaster::new(
        42,
        0x1122334455667788,
        Instant::now(),
        Duration::from_secs(2),
    )
    .unwrap()
}
fn no_datagram(socket: &UdpSocket) {
    socket.set_nonblocking(true).unwrap();
    let error = socket.recv(&mut [0; 513]).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
}
fn assert_service_closed(
    service: &Service,
    event: std::net::SocketAddr,
    general: std::net::SocketAddr,
) {
    assert_eq!(service.state(), State::Shutdown);
    assert!(service.event.is_none() && service.general.is_none());
    let _event = UdpSocket::bind(event).unwrap();
    let _general = UdpSocket::bind(general).unwrap();
}
#[test]
fn ptp_requires_both_authentication_and_concrete_authorization() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_receiver, _) = listener.accept().unwrap();
    let mut wire = Wire::mock(socket, Duration::from_secs(1), "device:112233445566");
    let _approval = wire.ptp_test_authorization().unwrap();
    assert!(wire.ptp_lease().is_err()); // allowlist alone is not authentication
    assert!(wire
        .start_ptp(
            42,
            0x1122334455667788,
            crate::receiver::lifecycle::TimingPeer::select(
                &[plist::Value::String("127.0.0.1".into())],
                "127.0.0.1".parse().unwrap()
            )
            .unwrap(),
            Duration::from_secs(1)
        )
        .is_err());
    let mut wire = wire
        .secure(
            ControlKeys::new(b"ptp-fixture"),
            ControlKeys::events(b"ptp-fixture"),
        )
        .unwrap();
    assert!(wire.ptp_lease().is_ok());
    wire.close();
    assert!(wire.ptp_lease().is_err());

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_receiver, _) = listener.accept().unwrap();
    let mut wire = Wire::mock(socket, Duration::from_secs(1), "device:112233445566")
        .secure(
            ControlKeys::new(b"ptp-fixture"),
            ControlKeys::events(b"ptp-fixture"),
        )
        .unwrap();
    assert!(wire.ptp_lease().is_err()); // keys alone cannot use legacy None leases
    assert!(wire
        .start_ptp(
            42,
            0x1122334455667788,
            crate::receiver::lifecycle::TimingPeer::select(
                &[plist::Value::String("127.0.0.1".into())],
                "127.0.0.1".parse().unwrap()
            )
            .unwrap(),
            Duration::from_secs(1)
        )
        .is_err());
    assert!(wire
        .buffered_timing("rtsp://127.0.0.1/session".into(), &fixture_clock())
        .is_err());
    // Direct control construction cannot bypass the same mandatory boundary.
    assert!(super::control::Control::new(
        &mut wire,
        "rtsp://127.0.0.1/session".into(),
        &fixture_clock()
    )
    .is_err());
}
#[test]
fn revoked_capability_rejects_attachment_and_releases_sockets_without_traffic() {
    let fixture = authorized_ptp_fixture();
    let lease = fixture.wire.ptp_lease().unwrap();
    let (event, peer_event) = udp_pair();
    let (general, peer_general) = udp_pair();
    let event_addr = event.local_addr().unwrap();
    let general_addr = general.local_addr().unwrap();
    fixture.revoke();
    assert!(fixture.wire.ptp_lease().is_err());
    assert!(Service::attach(fixture_clock(), event, general, lease).is_err());
    let _event = UdpSocket::bind(event_addr).unwrap();
    let _general = UdpSocket::bind(general_addr).unwrap();
    no_datagram(&peer_event);
    no_datagram(&peer_general);
}
#[test]
fn polling_revocation_blocks_scheduled_and_reply_datagrams_and_closes_immediately() {
    for change in 0..4 {
        let fixture = authorized_ptp_fixture();
        let (event, peer_event) = udp_pair();
        let (general, peer_general) = udp_pair();
        let event_addr = event.local_addr().unwrap();
        let general_addr = general.local_addr().unwrap();
        let mut service = Service::attach(
            fixture_clock(),
            event,
            general,
            fixture.wire.ptp_lease().unwrap(),
        )
        .unwrap();
        peer_event
            .send(&request(Kind::DelayReq, 1).encode().unwrap())
            .unwrap();
        match change {
            0 => fixture.revoke(),
            1 => std::fs::remove_file(fixture.approval.path().join("receiver-state/selected.toml"))
                .unwrap(),
            2 => {
                std::fs::write(fixture.approval.path().join("config.toml"), "invalid = [").unwrap()
            }
            _ => std::fs::write(
                fixture.approval.path().join("receiver-state/selected.toml"),
                "invalid = [",
            )
            .unwrap(),
        }
        assert!(service.poll(&AtomicBool::new(false)).is_err());
        no_datagram(&peer_event);
        no_datagram(&peer_general);
        assert_service_closed(&service, event_addr, general_addr);
        assert!(service.clock.last_peer.is_none()); // queued traffic was never processed
    }
}
#[test]
fn authorization_is_rechecked_for_each_outbound_datagram() {
    for kind in [
        Kind::Sync,
        Kind::FollowUp,
        Kind::DelayResp,
        Kind::PdelayResp,
        Kind::PdelayFollowUp,
        Kind::Announce,
    ] {
        let fixture = authorized_ptp_fixture();
        let (event, peer_event) = udp_pair();
        let (general, peer_general) = udp_pair();
        let event_addr = event.local_addr().unwrap();
        let general_addr = general.local_addr().unwrap();
        let mut service = Service::attach(
            fixture_clock(),
            event,
            general,
            fixture.wire.ptp_lease().unwrap(),
        )
        .unwrap();
        // A prior successful send must not cache permission for the next datagram.
        let first = Packet::new(Kind::Sync, 42, 0, vec![0; 10]);
        service.send(&first).unwrap();
        let mut received = [0; 512];
        let n = peer_event.recv(&mut received).unwrap();
        assert_eq!(Packet::decode(&received[..n]).unwrap().kind, Kind::Sync);
        fixture.revoke();
        let length = match kind {
            Kind::Sync | Kind::FollowUp => 10,
            Kind::Announce => 30,
            _ => 20,
        };
        let packet = Packet::new(kind, 42, 1, vec![0; length]);
        assert!(service.send(&packet).is_err());
        no_datagram(&peer_event);
        no_datagram(&peer_general);
        assert_service_closed(&service, event_addr, general_addr);
        // Restoring approval cannot resurrect an owner whose sockets were closed.
        std::fs::write(
            fixture.approval.path().join("config.toml"),
            "receiver_allowlist=['device:112233445566']",
        )
        .unwrap();
        assert!(service.poll(&AtomicBool::new(false)).is_err());
        assert!(service.send(&packet).is_err());
        no_datagram(&peer_event);
        no_datagram(&peer_general);
    }
}
#[test]
fn matching_socket_peers_cannot_target_another_receiver_with_a_valid_capability() {
    let fixture = authorized_ptp_fixture();
    let event = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let general = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    event.connect((Ipv4Addr::new(127, 0, 0, 2), 319)).unwrap();
    general.connect((Ipv4Addr::new(127, 0, 0, 2), 320)).unwrap();
    assert!(Service::attach(
        fixture_clock(),
        event,
        general,
        fixture.wire.ptp_lease().unwrap()
    )
    .is_err());
}

#[test]
fn timing_control_revocation_closes_without_sending_a_request() {
    let (wire, receiver) = mock_timing(|mut socket| {
        let mut byte = [0; 1];
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    });
    let mut wire = wire
        .secure(
            ControlKeys::new(b"timing-test"),
            ControlKeys::events(b"timing-test"),
        )
        .unwrap();
    let approval = wire.ptp_test_authorization().unwrap();
    let clock = fixture_clock();
    {
        let mut control = wire
            .buffered_timing("rtsp://127.0.0.2/session".into(), &clock)
            .unwrap();
        std::fs::write(approval.path().join("config.toml"), "receiver_allowlist=[]").unwrap();
        assert!(control.set_peers().is_err());
        assert!(control.set_peers().is_err());
    }
    assert!(wire.is_invalidated());
    drop(wire);
    receiver.join().unwrap();
}

#[test]
fn apple_follow_up_known_answer_and_conflicting_extensions() {
    // Independent literal: 34-byte header, timestamp 1.5 s, IEEE TLV (28)
    // then Apple OUI/subtype 4 (16): clock identity + two padding bytes.
    let hex = concat!(
        "1802006000000400000000000000000000000000",
        "11223344556677888005123402fd",
        "0000000000011dcd6500",
        "0003001c0080c200000100000000000000000000000000000000000000000000",
        "00030010000d9300000411223344556677880000"
    );
    let bytes: Vec<u8> = hex
        .as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect();
    let p = Packet::decode(&bytes).unwrap();
    assert_eq!(p.kind, Kind::FollowUp);
    assert_eq!(p.sequence, 0x1234);
    assert_eq!(
        p.timestamp().unwrap(),
        Timestamp::new(1, 500_000_000).unwrap()
    );
    assert_eq!(p.encode().unwrap(), bytes);
    for (index, byte) in [(79, 14), (86, 0xff), (95, 1), (32, 4)] {
        let mut bad = bytes.clone();
        bad[index] = byte;
        assert!(Packet::decode(&bad).is_err(), "index {index}");
    }
    let mut duplicate = p.clone();
    duplicate
        .body
        .extend_from_slice(&p.body[p.body.len() - 20..]);
    assert!(duplicate.encode().is_err());
    let now = Instant::now();
    let mut c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(2)).unwrap();
    c.check(now, false).unwrap();
    let mut sync = request(Kind::Sync, 0x1234);
    sync.flags |= 0x0200;
    c.receive(&sync.encode().unwrap(), now).unwrap();
    c.receive_general(&bytes, now).unwrap();
    assert_eq!(c.state(), State::PeerObserved);
    assert!(!c.media_permitted());
    peer(&mut c, now, 1);
    let mut wrong_port = p;
    wrong_port.source[9] ^= 1;
    assert!(c
        .receive_general(&wrong_port.encode().unwrap(), now)
        .is_err());
}

#[test]
fn apple_signaling_is_bounded_opaque_and_cannot_register_peers() {
    let mut body = vec![0xff; 10]; // IEEE wildcard target; no timestamp
                                   // Empirical Apple subtypes, kept opaque. No assertion about their meaning.
    for (subtype, size) in [(1, 22), (5, 32)] {
        body.extend_from_slice(&[0, 3, 0, size, 0, 0x0d, 0x93, 0, 0, subtype]);
        body.extend_from_slice(&[0, 0, 3, 1]);
        body.resize(body.len() + usize::from(size) - 10, 0);
    }
    let p = Packet::new(Kind::Signaling, 0x1122334455667788, 9, body);
    let encoded = p.encode().unwrap();
    assert_eq!(encoded.len(), 106);
    let now = Instant::now();
    let mut c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(1)).unwrap();
    c.check(now, false).unwrap();
    c.receive_general(&encoded, now).unwrap();
    assert_eq!(c.peer_ports[Kind::DelayReq as usize], None);
    assert_eq!(c.last_peer, Some(now));
    c.check(now + Duration::from_secs(1), false).unwrap();
    let mut bad = p.clone();
    bad.body[13] = 20;
    assert!(bad.encode().is_err());
    bad = p;
    bad.body[..10].copy_from_slice(&[0x22; 10]);
    assert!(c.receive_general(&bad.encode().unwrap(), now).is_err());
}

#[test]
fn announce_sequence_is_independent_and_quality_is_truthful() {
    let now = Instant::now();
    let mut c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(2)).unwrap();
    let first = c.due(now).unwrap();
    assert_eq!(first[0].body[14..18], [248, 0xfe, 0xff, 0xff]);
    assert_eq!(first[0].body[29], 0xa0);
    assert_eq!(first[0].flags & 0x003f, 0); // no UTC/traceability/timescale claims
    c.due(now + Duration::from_millis(125)).unwrap();
    let second = c.due(now + Duration::from_secs(1)).unwrap();
    assert_eq!(second[0].kind, Kind::Announce);
    assert_eq!(second[0].sequence, 1);
    assert_eq!(second[1].sequence, 2);
}

#[test]
fn connected_udp_rejects_unsolicited_source_and_authenticated_clock_mismatch() {
    let fixture = authorized_ptp_fixture();
    let (event, peer_event) = udp_pair();
    let (general, _peer_general) = udp_pair();
    let destination = event.local_addr().unwrap();
    let mut service = Service::attach(
        fixture_clock(),
        event,
        general,
        fixture.wire.ptp_lease().unwrap(),
    )
    .unwrap();
    let attacker = UdpSocket::bind((Ipv4Addr::new(127, 0, 0, 2), 0)).unwrap();
    let valid = request(Kind::DelayReq, 1).encode().unwrap();
    attacker.send_to(&valid, destination).unwrap();
    service.poll(&AtomicBool::new(false)).unwrap();
    assert!(service.clock.last_peer.is_none());
    assert_eq!(service.evidence().delay_requests, 0);
    let mut wrong = request(Kind::DelayReq, 2);
    wrong.source[0] ^= 1;
    peer_event.send(&wrong.encode().unwrap()).unwrap();
    service.poll(&AtomicBool::new(false)).unwrap();
    assert!(service.clock.last_peer.is_none());
    assert_eq!(service.evidence().delay_requests, 0);
    peer_event.send(&valid).unwrap();
    service.poll(&AtomicBool::new(false)).unwrap();
    assert!(service.clock.last_peer.is_some());
    assert_eq!(service.evidence().delay_requests, 1);
    assert_eq!(service.evidence().delay_responses_sent, 1);
    assert!(!service.clock.media_permitted());
}

// Reconstructed from reported Stua firmware 27.2 header fields, not a capture.
// Identity and remaining valid fields are synthetic; never encode this fixture.
fn stua_delay_req(sequence: u16) -> [u8; 44] {
    let mut bytes = [
        0x11, 2, 0, 44, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x11, 0x22, 0x33, 0x44,
        0x55, 0x66, 0x77, 0x88, 0x80, 5, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    bytes[30..32].copy_from_slice(&sequence.to_be_bytes());
    bytes
}

#[test]
fn stua_zero_control_delay_req_raw_regression() {
    for sequence in 1..=9 {
        let bytes = stua_delay_req(sequence);
        let decoded = Packet::decode(&bytes).unwrap();
        assert_eq!(decoded.kind, Kind::DelayReq);
        assert_eq!(decoded.sequence, sequence);
        let mut standard = bytes;
        standard[32] = 1;
        assert_eq!(Packet::decode(&standard).unwrap(), decoded);
        // Encoding continues to use the IEEE legacy value.
        assert_eq!(decoded.encode().unwrap()[32], 1);
    }
}

#[test]
fn control_field_compatibility_is_message_specific() {
    for kind in [
        Kind::Sync,
        Kind::DelayReq,
        Kind::PdelayReq,
        Kind::PdelayResp,
        Kind::FollowUp,
        Kind::DelayResp,
        Kind::PdelayFollowUp,
        Kind::Announce,
        Kind::Signaling,
    ] {
        let packet = Packet::new(
            kind,
            42,
            7,
            vec![
                0;
                match kind {
                    Kind::Sync | Kind::DelayReq | Kind::FollowUp | Kind::Signaling => 10,
                    Kind::Announce => 30,
                    _ => 20,
                }
            ],
        );
        let mut bytes = packet.encode().unwrap();
        let standard = bytes[32];
        for control in 0..=255 {
            bytes[32] = control;
            let allowed = control == standard
                || (control == 0
                    && matches!(
                        kind,
                        Kind::DelayReq | Kind::FollowUp | Kind::DelayResp | Kind::Announce
                    ));
            assert_eq!(
                Packet::decode(&bytes).is_ok(),
                allowed,
                "{kind:?} {control}"
            );
        }
    }
}

#[test]
fn stua_delay_exchange_preserves_validation_and_timestamp() {
    let now = Instant::now();
    let mut clock = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(2)).unwrap();
    for sequence in 1..=9 {
        let received = now + Duration::from_nanos(u64::from(sequence) * 123_456_789);
        let bytes = stua_delay_req(sequence);
        // Invalid requests must neither consume the sequence nor pin an identity.
        for (index, value) in [
            (0, 0x21),
            (1, 1),
            (3, 45),
            (4, 1),
            (6, 2),
            (15, 1),
            (20, 0x12),
            (29, 6),
            (32, 2),
            (40, 255),
        ] {
            let mut bad = bytes;
            bad[index] = value;
            // First request has no pinned port yet; a zero port is always invalid.
            if index == 29 && sequence == 1 {
                bad[28] = 0;
                bad[29] = 0;
            }
            assert!(clock.receive(&bad, received).is_err(), "index {index}");
        }
        let response = clock.receive(&bytes, received).unwrap().remove(0);
        assert_eq!(response.kind, Kind::DelayResp);
        assert_eq!(response.sequence, sequence);
        assert_eq!(
            response.source,
            Packet::new(Kind::Sync, 42, 0, vec![0; 10]).source
        );
        assert_eq!(response.body[10..], bytes[20..30]);
        assert_eq!(
            response.timestamp().unwrap(),
            ts(i128::from(sequence) * 123_456_789)
        );
        assert_eq!(response.correction, 0);
        assert_eq!(response.flags, 0x0400);
        assert_eq!(
            Packet::decode(&response.encode().unwrap()).unwrap(),
            response
        );
        assert_eq!(clock.state(), State::PeerObserved);
        assert!(!clock.media_permitted());
        assert!(clock.receive(&bytes, received).is_err());
        let mut standard = bytes;
        standard[32] = 1;
        assert!(clock.receive(&standard, received).is_err()); // same replay slot
    }
}

#[test]
fn stua_delay_req_service_routes_response_to_general_channel() {
    let fixture = authorized_ptp_fixture();
    let (event, peer_event) = udp_pair();
    let (general, peer_general) = udp_pair();
    let mut service = Service::attach(
        fixture_clock(),
        event,
        general,
        fixture.wire.ptp_lease().unwrap(),
    )
    .unwrap();
    let cancel = AtomicBool::new(false);
    service.poll(&cancel).unwrap();
    let mut buffer = [0; 513];
    let n = peer_event.recv(&mut buffer).unwrap();
    assert_eq!(Packet::decode(&buffer[..n]).unwrap().kind, Kind::Sync);
    for expected in [Kind::Announce, Kind::FollowUp] {
        let n = peer_general.recv(&mut buffer).unwrap();
        assert_eq!(Packet::decode(&buffer[..n]).unwrap().kind, expected);
    }
    // Keep scheduled traffic out of this routing assertion.
    service.clock.next_sync = Instant::now() + Duration::from_secs(2);
    service.clock.next_announce = service.clock.next_sync;
    peer_general.send(&stua_delay_req(1)).unwrap();
    service.poll(&cancel).unwrap();
    assert_eq!(service.state(), State::Synchronizing);
    no_datagram(&peer_event);
    no_datagram(&peer_general);
    peer_general.set_nonblocking(false).unwrap();
    for sequence in 1..=9 {
        peer_event.send(&stua_delay_req(sequence)).unwrap();
        let before = service.clock.timestamp(Instant::now()).unwrap();
        service.poll(&cancel).unwrap();
        let after = service.clock.timestamp(Instant::now()).unwrap();
        let n = peer_general.recv(&mut buffer).unwrap();
        let response = Packet::decode(&buffer[..n]).unwrap();
        assert_eq!(response.kind, Kind::DelayResp);
        assert_eq!(response.sequence, sequence);
        assert_eq!(response.body[10..], stua_delay_req(sequence)[20..30]);
        assert!((before.ns()..=after.ns()).contains(&response.timestamp().unwrap().ns()));
        assert_eq!(service.state(), State::PeerObserved);
        assert!(!service.clock.media_permitted());
        no_datagram(&peer_event);
    }
    assert_eq!(service.diagnostics.exchanges, Diagnostics::LIMIT);
    // One wrong-channel request was rejected.
    assert_eq!(service.diagnostics.rejected, 1);
    // Authorization still wins over a valid queued zero-control request.
    peer_event.send(&stua_delay_req(10)).unwrap();
    fixture.revoke();
    assert!(service.poll(&cancel).is_err());
    assert_eq!(service.state(), State::Shutdown);
    no_datagram(&peer_general);
}

#[test]
fn ptp_debug_diagnostics_are_bounded_and_payload_free() {
    use crate::receiver::diagnostics::tests::capture;
    let emit = || {
        let mut diagnostics = Diagnostics::default();
        let bytes = b"secret-payload-not-for-logs";
        let reason = Packet::decode(bytes).unwrap_err();
        for sequence in 0..1000 {
            diagnostics.reject("event", bytes, &reason);
            diagnostics.reject("general", &[], &reason);
            diagnostics.exchange(sequence);
        }
        assert_eq!(diagnostics.rejected, Diagnostics::LIMIT);
        assert_eq!(diagnostics.exchanges, Diagnostics::LIMIT);
    };
    assert!(capture(tracing::Level::INFO, emit).is_empty());
    let output = capture(tracing::Level::DEBUG, emit);
    assert_eq!(output.lines().count(), 16);
    assert_eq!(output.matches("rejected PTP datagram").count(), 8);
    assert_eq!(
        output
            .matches("accepted Delay_Req; sent Delay_Resp")
            .count(),
        8
    );
    assert!(output.contains("invalid PTP size"));
    assert!(!output.contains("secret-payload-not-for-logs"));
    assert!(!output.contains("1122334455667788"));
}

#[test]
fn inbound_sync_observed_but_conflicting_timeline_never_adopted() {
    let now = Instant::now();
    let mut clock = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(5)).unwrap();
    peer(&mut clock, now, 1);
    let generation = clock.generation();
    let mut sync = request(Kind::Sync, 0x1234);
    sync.flags = 0x0608;
    sync.interval = -3;
    let sync = sync.encode().unwrap();
    // A receiver can emit its own two-step Sync while we remain fixed GM.
    assert!(clock
        .receive(&sync, now + Duration::from_secs(1))
        .unwrap()
        .is_empty());
    let mut follow = request(Kind::FollowUp, 0x1234);
    follow.flags = 0x0408;
    follow
        .body
        .extend_from_slice(&[0, 3, 0, 28, 0, 0x80, 0xc2, 0, 0, 1]);
    follow.body.extend_from_slice(&[0; 22]);
    follow
        .body
        .extend_from_slice(&[0, 3, 0, 16, 0, 0x0d, 0x93, 0, 0, 4]);
    follow
        .body
        .extend_from_slice(&0x1122334455667788u64.to_be_bytes());
    follow.body.extend_from_slice(&[0; 2]);
    let mut bytes = follow.encode().unwrap();
    // Hypothesis fixture, NOT a Stua capture: TLV names our timeline while
    // sourcePortIdentity names the receiver. This is not authenticated adoption.
    bytes[86..94].copy_from_slice(&42u64.to_be_bytes());
    for _ in 0..2 {
        assert!(clock
            .receive_general(&bytes, now + Duration::from_secs(1))
            .unwrap_err()
            .to_string()
            .contains("conflicting Apple clock TLV"));
    }
    assert!(clock
        .receive_general(&bytes[..95], now + Duration::from_secs(1))
        .is_err());
    assert!(clock
        .receive(
            &request(Kind::DelayReq, 1).encode().unwrap(),
            now + Duration::from_secs(2)
        )
        .is_err());
    let mut wrong = request(Kind::DelayReq, 2);
    wrong.source[..8].copy_from_slice(&43u64.to_be_bytes());
    assert!(clock
        .receive(&wrong.encode().unwrap(), now + Duration::from_secs(2))
        .is_err());
    assert_eq!(clock.generation(), generation);
    assert_eq!(clock.state(), State::PeerObserved);
    assert!(!clock.media_permitted());
    assert!(clock
        .check(now + Duration::from_millis(4001), false)
        .is_err());
    assert_eq!(clock.state(), State::Lost);
}

#[test]
fn session_timing_cancellation_revocation_and_control_failure_release_resources() {
    use super::session::{Phase, TimingSession};
    for failure in 0..3 {
        let fixture = authorized_ptp_fixture();
        let (event, _peer_event) = udp_pair();
        let (general, _peer_general) = udp_pair();
        let addresses = (event.local_addr().unwrap(), general.local_addr().unwrap());
        let service = Service::attach(
            fixture_clock(),
            event,
            general,
            fixture.wire.ptp_lease().unwrap(),
        )
        .unwrap();
        let mut owner = TimingSession::attach(service);
        assert_eq!(owner.phase(), Phase::Observing);
        let cancel = AtomicBool::new(failure == 0);
        if failure == 1 {
            fixture.revoke();
        }
        let result = if failure == 2 {
            owner.during_control(&cancel, || -> Result<()> {
                anyhow::bail!("control rejected")
            })
        } else {
            owner.during_control(&cancel, || -> Result<()> {
                panic!("invalid session cannot issue control")
            })
        };
        assert!(result.is_err());
        assert_eq!(owner.phase(), Phase::Failed);
        assert_eq!(owner.state(), State::Shutdown);
        assert!(owner.poll(&AtomicBool::new(false)).is_err());
        owner.shutdown();
        assert_eq!(owner.phase(), Phase::Failed);
        let _event = UdpSocket::bind(addresses.0).unwrap();
        let _general = UdpSocket::bind(addresses.1).unwrap();
    }
}

#[test]
fn session_timing_drop_and_repeated_shutdown_close_both_sockets() {
    use super::session::{Phase, TimingSession};
    for explicit in [false, true] {
        let fixture = authorized_ptp_fixture();
        let (event, _peer_event) = udp_pair();
        let (general, _peer_general) = udp_pair();
        let addresses = (event.local_addr().unwrap(), general.local_addr().unwrap());
        let service = Service::attach(
            fixture_clock(),
            event,
            general,
            fixture.wire.ptp_lease().unwrap(),
        )
        .unwrap();
        let mut owner = TimingSession::attach(service);
        if explicit {
            owner.shutdown();
            owner.shutdown();
            assert_eq!(owner.phase(), Phase::Closed);
        }
        drop(owner);
        let _event = UdpSocket::bind(addresses.0).unwrap();
        let _general = UdpSocket::bind(addresses.1).unwrap();
    }
}

#[test]
fn apple_tv_single_delay_then_ten_seconds_of_validated_observations() {
    let now = Instant::now();
    let mut c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(2)).unwrap();
    peer(&mut c, now, 1);
    let generation = c.generation();
    for second in 1..=10 {
        let at = now + Duration::from_secs(second);
        let mut sync = request(Kind::Sync, second as u16);
        sync.source[8..].copy_from_slice(&1u16.to_be_bytes());
        sync.flags |= 0x0200;
        assert!(c.receive(&sync.encode().unwrap(), at).unwrap().is_empty());
        let mut follow = request(Kind::FollowUp, second as u16);
        follow.source = sync.source;
        c.receive_general(&follow.encode().unwrap(), at).unwrap();
        assert!(c.receive_general(&follow.encode().unwrap(), at).is_err());
        let mut announce = Packet::new(Kind::Announce, c.peer_clock_id, second as u16, vec![0; 30]);
        announce.source[8..].copy_from_slice(&2u16.to_be_bytes());
        // An announcement naming another GM is observed, never elected.
        announce.body[19..27].copy_from_slice(&99u64.to_be_bytes());
        c.receive_general(&announce.encode().unwrap(), at).unwrap();
        let mut signaling = request(Kind::Signaling, second as u16);
        signaling.source[8..].copy_from_slice(&3u16.to_be_bytes());
        c.receive_general(&signaling.encode().unwrap(), at).unwrap();
        assert_eq!(c.last_peer, Some(at));
        c.check(at, false).unwrap();
        assert_eq!(c.state(), State::PeerObserved);
        assert_eq!(c.clock_id, 42);
        assert_eq!(c.generation(), generation);
        assert!(!c.media_permitted());
    }
    // Rejected identities, changed role ports, wrong targets and replay cannot
    // keep a silent receiver alive. Poll continuously to isolate peer expiry.
    for second in 11..=13 {
        let at = now + Duration::from_secs(second);
        let mut bad = request(Kind::Sync, second as u16);
        assert!(c.receive(&bad.encode().unwrap(), at).is_err()); // role port changed
        bad.source[..8].copy_from_slice(&99u64.to_be_bytes());
        assert!(c.receive(&bad.encode().unwrap(), at).is_err());
        c.check(at, false).unwrap();
        assert_eq!(c.last_peer, Some(now + Duration::from_secs(10)));
    }
    assert!(c.check(now + Duration::from_millis(13001), false).is_err());
    assert_eq!(c.state(), State::Lost);
}

#[test]
fn unmatched_follow_up_and_wrong_channel_cannot_establish_liveness() {
    let now = Instant::now();
    let mut c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(2)).unwrap();
    let f = request(Kind::FollowUp, 1).encode().unwrap();
    assert!(c.receive_general(&f, now).is_err());
    assert!(c.receive(&f, now).is_err());
    let sync = request(Kind::Sync, 1).encode().unwrap();
    assert!(c.receive_general(&sync, now).is_err());
    assert!(c.last_peer.is_none());
    assert!(c.check(now + Duration::from_secs(2), false).is_err());
}

#[test]
fn service_reports_each_validated_message_and_rejection_separately() {
    let fixture = authorized_ptp_fixture();
    let (event, peer_event) = udp_pair();
    let (general, peer_general) = udp_pair();
    let mut service = Service::attach(
        fixture_clock(),
        event,
        general,
        fixture.wire.ptp_lease().unwrap(),
    )
    .unwrap();
    let mut sync = request(Kind::Sync, 7);
    sync.flags |= 0x0200;
    sync.source[8..].copy_from_slice(&1u16.to_be_bytes());
    peer_event.send(&sync.encode().unwrap()).unwrap();
    peer_event
        .send(&request(Kind::DelayReq, 1).encode().unwrap())
        .unwrap();
    let mut follow = request(Kind::FollowUp, 7);
    follow.source = sync.source;
    peer_general.send(&follow.encode().unwrap()).unwrap();
    peer_general
        .send(
            &Packet::new(Kind::Announce, 0x1122334455667788, 1, vec![0; 30])
                .encode()
                .unwrap(),
        )
        .unwrap();
    let mut signaling = request(Kind::Signaling, 1);
    signaling.body = vec![0xff; 10];
    peer_general.send(&signaling.encode().unwrap()).unwrap();
    peer_general.send(&signaling.encode().unwrap()).unwrap(); // replay
    peer_general.send(&[0; 12]).unwrap(); // malformed
    service.poll(&AtomicBool::new(false)).unwrap();
    let evidence = service.evidence();
    assert_eq!(evidence.sync_received, 1);
    assert_eq!(evidence.follow_up_received, 1);
    assert_eq!(evidence.announce_received, 1);
    assert_eq!(evidence.signaling_received, 1);
    assert_eq!(evidence.delay_requests, 1);
    assert_eq!(evidence.delay_responses_sent, 1);
    assert_eq!(evidence.general_received, 3);
    assert_eq!(evidence.rejected, 2);
    assert_eq!(service.clock.clock_id, 42);
    assert!(!service.clock.media_permitted());
}
