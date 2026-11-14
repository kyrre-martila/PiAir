use super::*;
use std::net::{Ipv4Addr, SocketAddr};
use time::Timestamp;
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
fn timestamp_precision_and_rollover() {
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
}
#[test]
fn grandmaster_liveness_is_not_receiver_lock() {
    let now = Instant::now();
    let mut c = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(2)).unwrap();
    assert_eq!(c.state(), State::Initializing);
    let packets = c.due(now).unwrap();
    assert_eq!(packets.len(), 3);
    let sync = &packets[2];
    assert_eq!(sync.kind, Kind::Sync);
    assert_eq!(sync.flags, 0x0608);
    let fup = c.follow_up(sync, now + Duration::from_nanos(123)).unwrap();
    assert_eq!(fup.sequence, sync.sequence);
    assert_eq!(fup.timestamp().unwrap().nanos, 123);
    assert!(c.follow_up(sync, now + Duration::from_nanos(124)).is_err());
    assert_eq!(packets[0].body[14], 248); // never claims GPS quality
    peer(&mut c, now + Duration::from_millis(1), 65535);
    peer(&mut c, now + Duration::from_millis(2), 0); // sequence wrap
    assert_eq!(c.state(), State::PeerObserved);
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
    assert_eq!(response[0].flags, 0x0608);
    assert_eq!(response[0].kind, Kind::PdelayResp);
    assert_eq!(response[0].body[10..], request(Kind::PdelayReq, 12).source);
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
    for _ in 0..4 {
        let n = peer_general.recv(&mut b).unwrap();
        kinds.push(Packet::decode(&b[..n]).unwrap().kind);
    }
    assert_eq!(
        kinds,
        vec![
            Kind::Announce,
            Kind::Signaling,
            Kind::FollowUp,
            Kind::DelayResp
        ]
    );
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
use std::net::{TcpListener, TcpStream};

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
    for (index, byte) in [(79, 14), (95, 1), (32, 4)] {
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
    assert!(c
        .ports
        .values()
        .all(|p| p.sequences[Kind::DelayReq as usize].is_none()));
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
    assert_eq!(first[0].flags & 0x003f, 8); // PTP timescale, no UTC/traceability claims
    c.due(now + Duration::from_millis(125)).unwrap();
    let second = c.due(now + Duration::from_secs(1)).unwrap();
    assert_eq!(second[0].kind, Kind::Announce);
    assert_eq!(second[0].sequence, 1);
    assert_eq!(second[2].sequence, 2);
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
            (29, 0),
            (32, 2),
            (40, 255),
        ] {
            let mut bad = bytes;
            bad[index] = value;
            // First request has no pinned port yet; a zero port is always invalid.
            if index == 29 {
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
        assert_eq!(response.flags, 0x0608);
        assert_eq!(
            Packet::decode(&response.encode().unwrap()).unwrap(),
            response
        );
        assert_eq!(clock.state(), State::PeerObserved);
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
    for expected in [Kind::Announce, Kind::Signaling, Kind::FollowUp] {
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
    clock
        .receive_general(&bytes, now + Duration::from_secs(1))
        .unwrap();
    // Clock identity in a private TLV is informational, never authority or lock.
    assert!(clock
        .receive_general(&bytes, now + Duration::from_secs(1))
        .is_err());
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
    assert_eq!(clock.state(), State::PeerObserved);
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
    }
    // Rejected identities, changed role ports, wrong targets and replay cannot
    // keep a silent receiver alive. Poll continuously to isolate peer expiry.
    for second in 11..=13 {
        let at = now + Duration::from_secs(second);
        let mut bad = request(Kind::Sync, second as u16);
        bad.source[8..].fill(0);
        assert!(bad.encode().is_err());
        bad = request(Kind::Sync, second as u16);
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
}

#[test]
fn zero_source_port_is_rejected_before_any_observation() {
    let now = Instant::now();
    for kind in [
        Kind::DelayReq,
        Kind::PdelayReq,
        Kind::Sync,
        Kind::FollowUp,
        Kind::Announce,
        Kind::Signaling,
    ] {
        let length = match kind {
            Kind::PdelayReq => 20,
            Kind::Announce => 30,
            _ => 10,
        };
        let mut packet = Packet::new(kind, 0x1122334455667788, 7, vec![0; length]);
        let mut bytes = packet.encode().unwrap();
        bytes[28..30].fill(0); // independent wire mutation bypasses encoder validation
        packet.source[8..].fill(0);
        assert_eq!(packet.port_number(), 0);
        assert!(packet.encode().is_err());
        assert!(Packet::decode(&bytes).is_err());
        let mut clock = fixture_clock();
        // Defense at the typed observation boundary as well as in the decoder.
        assert!(clock.validate_observation(&packet, now).is_err());
        assert!(clock.last_peer.is_none());
        assert!(clock.ports.is_empty());
        // A fresh clock uses this test's deterministic time base.
        let mut clock =
            Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(2)).unwrap();
        let result = if kind.event() {
            clock.receive(&bytes, now).map(|_| ())
        } else {
            clock.receive_general(&bytes, now)
        };
        assert!(result.is_err());
        assert!(clock.last_peer.is_none());
    }
}

#[test]
fn unsupported_and_malformed_sync_cannot_pin_or_refresh_peer() {
    let now = Instant::now();
    let mut clock = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(2)).unwrap();
    peer(&mut clock, now, 1);
    let sync = request(Kind::Sync, 9);
    let valid = sync.encode().unwrap();
    let mut rejected = Vec::new();
    for correction in [-1i64, 1, i64::MAX] {
        let mut bytes = valid.clone();
        bytes[8..16].copy_from_slice(&correction.to_be_bytes());
        assert!(Packet::decode(&bytes).is_ok()); // valid wire form, unsupported policy
        rejected.push(bytes);
    }
    for bit in 0..16 {
        if (1u16 << bit) & 0x0608 != 0 {
            continue;
        }
        let mut bytes = valid.clone();
        bytes[6..8].copy_from_slice(&(sync.flags | (1u16 << bit)).to_be_bytes());
        rejected.push(bytes);
    }
    let mut extended = valid.clone();
    extended.extend_from_slice(&[0, 0x7f, 0, 0]); // bounded unknown TLV
    extended[2..4].copy_from_slice(&48u16.to_be_bytes());
    assert!(Packet::decode(&extended).is_ok());
    rejected.push(extended);
    for (index, value) in [(1, 1), (4, 1), (16, 1), (32, 1), (40, 255)] {
        let mut bytes = valid.clone();
        bytes[index] = value;
        rejected.push(bytes);
    }
    rejected.push(valid[..43].to_vec());
    for bytes in rejected {
        assert!(clock.receive(&bytes, now + Duration::from_secs(1)).is_err());
        assert_eq!(clock.last_peer, Some(now));
        assert!(clock
            .ports
            .values()
            .all(|p| p.sequences[Kind::Sync as usize].is_none() && p.sync.is_none()));
        assert_eq!(clock.clock_id, 42);
    }
    // Invalid Sync did not consume this sequence or prevent a supported one.
    assert!(clock
        .receive(&valid, now + Duration::from_secs(1))
        .unwrap()
        .is_empty());
    assert_eq!(clock.last_peer, Some(now + Duration::from_secs(1)));
    for second in 2..=4 {
        clock
            .check(now + Duration::from_secs(second), false)
            .unwrap();
    }
    assert!(clock
        .check(now + Duration::from_millis(4001), false)
        .is_err());
}

#[test]
fn multiple_ports_in_same_role_keep_independent_sequences_and_sync_pairs() {
    let now = Instant::now();
    let mut clock = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(2)).unwrap();
    // Reported Apple TV port number, plus concurrent identities on that ClockID.
    // This is a regression model, not a byte-for-byte hardware capture.
    for port in [32969u16, 1, 0x8005] {
        let mut sync = request(Kind::Sync, 65535);
        sync.source[8..].copy_from_slice(&port.to_be_bytes());
        sync.flags |= 0x0200;
        clock.receive(&sync.encode().unwrap(), now).unwrap();
    }
    for port in [1u16, 32969, 0x8005] {
        let mut follow = request(Kind::FollowUp, 65535);
        follow.source[8..].copy_from_slice(&port.to_be_bytes());
        clock
            .receive_general(&follow.encode().unwrap(), now)
            .unwrap();
        assert!(clock
            .receive_general(&follow.encode().unwrap(), now)
            .is_err());
        let mut delay = request(Kind::DelayReq, 65535);
        delay.source = follow.source;
        let response = clock
            .receive(&delay.encode().unwrap(), now)
            .unwrap()
            .remove(0);
        assert_eq!(response.body[10..], delay.source);
        assert!(clock.receive(&delay.encode().unwrap(), now).is_err());
        delay.sequence = 0;
        clock.receive(&delay.encode().unwrap(), now).unwrap();
        delay.sequence = 65534;
        assert!(clock.receive(&delay.encode().unwrap(), now).is_err());
    }
    assert_eq!(clock.ports.len(), 3);
    assert_eq!(clock.clock_id, 42);
}

#[test]
fn outgoing_signaling_has_reference_wire_layout_and_independent_sequence() {
    let now = Instant::now();
    let mut clock = Grandmaster::new(42, 0x1122334455667788, now, Duration::from_secs(2)).unwrap();
    let due = clock.due(now).unwrap();
    let signal = due.iter().find(|p| p.kind == Kind::Signaling).unwrap();
    let bytes = signal.encode().unwrap();
    assert_eq!(bytes.len(), 106);
    assert_eq!(&bytes[..8], &[0x1c, 2, 0, 106, 0, 0, 4, 8]);
    assert_eq!(&bytes[28..34], &[0x80, 5, 0, 0, 5, 128]);
    assert_eq!(&bytes[34..44], &[0; 10]);
    assert_eq!(
        &bytes[44..58],
        &[0, 3, 0, 22, 0, 13, 147, 0, 0, 1, 0, 0, 3, 1]
    );
    assert_eq!(
        &bytes[70..84],
        &[0, 3, 0, 32, 0, 13, 147, 0, 0, 5, 0, 0, 3, 1]
    );
    let next = clock.due(now + Duration::from_secs(1)).unwrap();
    assert_eq!(
        next.iter()
            .find(|p| p.kind == Kind::Signaling)
            .unwrap()
            .sequence,
        1
    );
}

#[test]
fn pdelay_two_step_service_echoes_identity_and_post_send_timestamp() {
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
    let mut req = request(Kind::PdelayReq, 42);
    req.source[8..].copy_from_slice(&32969u16.to_be_bytes());
    peer_event.send(&req.encode().unwrap()).unwrap();
    service.poll(&AtomicBool::new(false)).unwrap();
    let mut bytes = [0; 512];
    let mut response = None;
    for _ in 0..2 {
        let n = peer_event.recv(&mut bytes).unwrap();
        let packet = Packet::decode(&bytes[..n]).unwrap();
        if packet.kind == Kind::PdelayResp {
            response = Some(packet);
        }
    }
    let response = response.unwrap();
    assert_eq!(response.flags, 0x0608);
    assert_eq!(response.interval, -3);
    assert_eq!(response.sequence, 42);
    assert_eq!(&response.body[10..], &req.source);
    let mut follow = None;
    for _ in 0..4 {
        let n = peer_general.recv(&mut bytes).unwrap();
        let packet = Packet::decode(&bytes[..n]).unwrap();
        if packet.kind == Kind::PdelayFollowUp {
            follow = Some(packet);
        }
    }
    let follow = follow.unwrap();
    assert_eq!(follow.flags, 0x0408);
    assert_eq!(follow.interval, -3);
    assert_eq!(follow.sequence, 42);
    assert_eq!(&follow.body[10..], &req.source);
    assert!(follow.timestamp().unwrap().ns() >= response.timestamp().unwrap().ns());
    assert_eq!(service.evidence().pdelay_requests, 1);
    assert_eq!(service.evidence().pdelay_follow_up_sent, 1);
}
