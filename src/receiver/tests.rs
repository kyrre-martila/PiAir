//! Loopback only: deterministic receiver-side keys/SRP private material. Client
//! randomness remains OS generated. No mDNS or real receivers used by these tests.
use super::*;
use super::{crypto::*, store::Credential, tlv::*, wire::Wire};
use ::srp::{client::SrpClient, groups::G_3072, server::SrpServer};
use ed25519_dalek::{Signer, SigningKey};
use num_bigint::BigUint;
use sha2::Sha512;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    thread,
    time::Duration,
};
use subtle::ConstantTimeEq;
use x25519_dalek::{PublicKey, StaticSecret};

const ID: &str = "device:aabbccddeeff";
const PI: &str = "12345678-1234-1234-1234-123456789ABC";
fn credential() -> Credential {
    Credential {
        version: 1,
        receiver_id: ID.into(),
        pairing_id: PI.into(),
        controller_id: "00000000-0000-4000-8000-000000000001".into(),
        seed: [11; 32],
        receiver_key: SigningKey::from_bytes(&[22; 32]).verifying_key().to_bytes(),
    }
}
fn request(socket: &mut TcpStream, expected: &str) -> Vec<u8> {
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let mut b = [0];
        socket.read_exact(&mut b).unwrap();
        header.push(b[0]);
    }
    let text = String::from_utf8(header).unwrap();
    assert!(text.starts_with(&format!("POST {expected} RTSP/1.0\r\n")));
    assert!(text.contains("X-Apple-HKP: 3\r\n"));
    let length = text
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length: "))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut body = vec![0; length];
    socket.read_exact(&mut body).unwrap();
    body
}
fn reply(socket: &mut TcpStream, seq: u32, fields: &[(u8, &[u8])]) {
    let body = encode(fields);
    write!(
        socket,
        "RTSP/1.0 200 OK\r\nCSeq: {seq}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .unwrap();
    socket.write_all(&body).unwrap();
}
fn local_mock(f: impl FnOnce(TcpStream) + Send + 'static) -> (Wire, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let handle = thread::spawn(move || f(listener.accept().unwrap().0));
    (Wire::mock(socket, Duration::from_secs(5), ID), handle)
}
#[derive(Clone, Copy)]
enum Fault {
    None,
    WrongPin,
    Proof,
    Signature,
    Identity,
    State,
    Tag,
}
fn setup_mock(mut socket: TcpStream, fault: Fault) {
    assert!(request(&mut socket, "/pair-pin-start").is_empty());
    reply(&mut socket, 1, &[]);
    let m1 = Tlv::parse(&request(&mut socket, "/pair-setup")).unwrap();
    assert_eq!(m1.get(0).unwrap(), [0]);
    let salt = [7; 16];
    let private = [9; 32];
    let verifier =
        SrpClient::<Sha512>::new(&G_3072).compute_verifier(b"Pair-Setup", b"1234", &salt);
    let server = SrpServer::<Sha512>::new(&G_3072);
    let b = srp::pad(&server.compute_public_ephemeral(&private, &verifier)).unwrap();
    reply(
        &mut socket,
        2,
        &[(STATE, &[2]), (SALT, &salt), (PUBLIC_KEY, &b)],
    );
    let m3 = Tlv::parse(&request(&mut socket, "/pair-setup")).unwrap();
    let a = m3.get(PUBLIC_KEY).unwrap();
    let u = BigUint::from_bytes_be(hash(&[a, &b]).as_ref());
    let shared = server.compute_premaster_secret(
        &BigUint::from_bytes_be(a),
        &BigUint::from_bytes_be(&verifier),
        &u,
        &BigUint::from_bytes_be(&private),
    );
    let key = hash(&[&shared.to_bytes_be()]);
    let (m1, m2) = srp::proofs(a, &b, &salt, key.as_ref());
    if matches!(fault, Fault::WrongPin) {
        assert!(!bool::from(m1.as_ref().ct_eq(m3.get(PROOF).unwrap())));
        reply(&mut socket, 3, &[(STATE, &[4]), (7, &[2])]);
        return;
    }
    assert_eq!(m3.get(PROOF).unwrap(), m1.as_ref());
    if matches!(fault, Fault::Proof) {
        reply(&mut socket, 3, &[(STATE, &[4]), (PROOF, &[0; 64])]);
        return;
    }
    if matches!(fault, Fault::State) {
        reply(&mut socket, 3, &[(STATE, &[6]), (PROOF, m2.as_ref())]);
        return;
    }
    reply(&mut socket, 3, &[(STATE, &[4]), (PROOF, m2.as_ref())]);
    let m5 = Tlv::parse(&request(&mut socket, "/pair-setup")).unwrap();
    let encryption = derive(
        key.as_ref(),
        "Pair-Setup-Encrypt-Salt",
        "Pair-Setup-Encrypt-Info",
    );
    let plaintext = open(
        &encryption,
        &nonce(b"PS-Msg05"),
        m5.get(ENCRYPTED).unwrap(),
        &[],
    )
    .unwrap();
    let controller = Tlv::parse(&plaintext).unwrap();
    let x = derive(
        key.as_ref(),
        "Pair-Setup-Controller-Sign-Salt",
        "Pair-Setup-Controller-Sign-Info",
    );
    let info = [
        x.as_ref(),
        controller.get(IDENTIFIER).unwrap(),
        controller.get(PUBLIC_KEY).unwrap(),
    ]
    .concat();
    crypto::verify(
        &controller.array(PUBLIC_KEY).unwrap(),
        &info,
        controller.get(SIGNATURE).unwrap(),
    )
    .unwrap();
    let signing = SigningKey::from_bytes(&[22; 32]);
    let public = signing.verifying_key().to_bytes();
    let id = if matches!(fault, Fault::Identity) {
        "OTHER-IDENTITY"
    } else {
        PI
    };
    let x = derive(
        key.as_ref(),
        "Pair-Setup-Accessory-Sign-Salt",
        "Pair-Setup-Accessory-Sign-Info",
    );
    let info = [x.as_ref(), id.as_bytes(), &public].concat();
    let mut signature = signing.sign(&info).to_bytes();
    if matches!(fault, Fault::Signature) {
        signature[0] ^= 1;
    }
    let sub = encode(&[
        (IDENTIFIER, id.as_bytes()),
        (PUBLIC_KEY, &public),
        (SIGNATURE, &signature),
    ]);
    let mut encrypted = seal(&encryption, &nonce(b"PS-Msg06"), &sub, &[]).unwrap();
    if matches!(fault, Fault::Tag) {
        encrypted[0] ^= 1;
    }
    reply(&mut socket, 4, &[(STATE, &[6]), (ENCRYPTED, &encrypted)]);
}
#[test]
fn persistent_pin_setup_verifies_both_proofs_and_identity() {
    let (wire, server) = local_mock(|s| setup_mock(s, Fault::None));
    let c = pairing::setup(wire, ID, PI, || Ok(Zeroizing::new("1234".into()))).unwrap();
    assert_eq!(c.receiver_id, ID);
    assert_eq!(c.receiver_key, credential().receiver_key);
    c.validate(ID).unwrap();
    server.join().unwrap();
}
#[test]
fn setup_rejects_wrong_pin_proof_signature_identity_state_and_aead() {
    for fault in [
        Fault::WrongPin,
        Fault::Proof,
        Fault::Signature,
        Fault::Identity,
        Fault::State,
        Fault::Tag,
    ] {
        let (wire, server) = local_mock(move |s| setup_mock(s, fault));
        let pin = if matches!(fault, Fault::WrongPin) {
            "9999"
        } else {
            "1234"
        };
        assert!(pairing::setup(wire, ID, PI, || Ok(Zeroizing::new(pin.into()))).is_err());
        server.join().unwrap();
    }
}
fn verify_mock(mut socket: TcpStream, fault: Fault) {
    let m1 = Tlv::parse(&request(&mut socket, "/pair-verify")).unwrap();
    let controller_public: [u8; 32] = m1.array(PUBLIC_KEY).unwrap();
    let private = StaticSecret::from([33; 32]);
    let public = PublicKey::from(&private).to_bytes();
    let shared = private.diffie_hellman(&PublicKey::from(controller_public));
    let encryption = derive(
        shared.as_bytes(),
        "Pair-Verify-Encrypt-Salt",
        "Pair-Verify-Encrypt-Info",
    );
    let id = if matches!(fault, Fault::Identity) {
        "CHANGED-ID"
    } else {
        PI
    };
    let info = [&public[..], id.as_bytes(), &controller_public].concat();
    let signing = SigningKey::from_bytes(&[22; 32]);
    let mut signature = signing.sign(&info).to_bytes();
    if matches!(fault, Fault::Signature) {
        signature[0] ^= 1;
    }
    let sub = encode(&[(IDENTIFIER, id.as_bytes()), (SIGNATURE, &signature)]);
    let mut encrypted = seal(&encryption, &nonce(b"PV-Msg02"), &sub, &[]).unwrap();
    if matches!(fault, Fault::Tag) {
        encrypted[0] ^= 1;
    }
    reply(
        &mut socket,
        1,
        &[
            (STATE, &[2]),
            (PUBLIC_KEY, &public),
            (ENCRYPTED, &encrypted),
        ],
    );
    if matches!(fault, Fault::Identity | Fault::Signature | Fault::Tag) {
        return;
    }
    let m3 = Tlv::parse(&request(&mut socket, "/pair-verify")).unwrap();
    let plain = open(
        &encryption,
        &nonce(b"PV-Msg03"),
        m3.get(ENCRYPTED).unwrap(),
        &[],
    )
    .unwrap();
    let sub = Tlv::parse(&plain).unwrap();
    let info = [
        &controller_public[..],
        sub.get(IDENTIFIER).unwrap(),
        &public,
    ]
    .concat();
    crypto::verify(
        &SigningKey::from_bytes(&[11; 32]).verifying_key().to_bytes(),
        &info,
        sub.get(SIGNATURE).unwrap(),
    )
    .unwrap();
    if matches!(fault, Fault::State) {
        reply(&mut socket, 2, &[(STATE, &[2])]);
    } else {
        reply(&mut socket, 2, &[(STATE, &[4])]);
    }
}
#[test]
fn pair_verify_success_and_failure_never_yield_unverified_keys() {
    for fault in [
        Fault::None,
        Fault::Identity,
        Fault::Signature,
        Fault::Tag,
        Fault::State,
    ] {
        let (wire, server) = local_mock(move |s| verify_mock(s, fault));
        assert_eq!(
            pairing::verify(wire, &credential()).is_ok(),
            matches!(fault, Fault::None)
        );
        server.join().unwrap();
    }
}
#[test]
fn framing_rejects_replay_tampering_truncation_and_overflow() {
    let mut client = ControlKeys::new(b"test secret");
    let mut server = ControlKeys::new(b"test secret");
    // Server has opposite read/write directions.
    server.swap_directions_for_test();
    let frame = client.encrypt(b"protocol fixture").unwrap();
    assert_eq!(&*server.decrypt(&frame).unwrap(), b"protocol fixture");
    assert!(server.decrypt(&frame).is_err());
    assert!(server.decrypt(&frame).is_err()); // poisoned after authentication failure
    for bad in [frame[..frame.len() - 1].to_vec(), {
        let mut b = frame.to_vec();
        b[5] ^= 1;
        b
    }] {
        let mut receiver = ControlKeys::new(b"test secret");
        receiver.swap_directions_for_test();
        assert!(receiver.decrypt(&bad).is_err());
        assert!(receiver.decrypt(&frame).is_err()); // cannot recover/reuse failed state
    }
    assert!(client.encrypt(&[0; 1025]).is_err());
}
#[test]
fn strict_response_parser_and_handshake_deadline() {
    for response in [
        "HTTP/1.1 302 Moved\r\nLocation: http://127.0.0.2/\r\nContent-Length: 0\r\n\r\n",
        "RTSP/1.0 200 OK\r\nCSeq: 2\r\nContent-Length: 0\r\n\r\n",
        "RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 999999\r\n\r\n",
        "RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx",
        "RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 20\r\n\r\nx",
        "RTSP/1.0 200 OK\r\nCSeq: 1\r\nTransfer-Encoding: chunked\r\nContent-Length: 0\r\n\r\n",
    ] {
        let (mut wire, server) = local_mock(move |mut s| {
            request(&mut s, "/pair-setup");
            s.write_all(response.as_bytes()).unwrap();
        });
        assert!(wire.post("/pair-setup", &[]).is_err());
        assert!(wire.post("/pair-verify", &[]).is_err()); // poisoned; no retry
        drop(wire);
        server.join().unwrap();
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_peer, _) = listener.accept().unwrap();
    let mut wire = Wire::mock(socket, Duration::from_millis(30), ID);
    assert!(wire.post("/pair-setup", &[]).is_err());
}
fn selection(port: u16) -> Selection {
    Selection {
        id: ID.into(),
        pairing_id: PI.into(),
        addresses: ["127.0.0.1".parse().unwrap()].into(),
        port,
        features: 1 << 46,
    }
}
fn snapshot(port: u16) -> Vec<Receiver> {
    use mdns_sd::ServiceInfo;
    let info = ServiceInfo::new(
        "_airplay._tcp.local.",
        "Fixture",
        "fixture.local.",
        "127.0.0.1",
        port,
        [
            ("deviceid", "AA:BB:CC:DD:EE:FF"),
            ("pi", PI),
            ("features", "0x0,0x4000"),
        ]
        .as_slice(),
    )
    .unwrap();
    vec![Receiver {
        id: ID.into(),
        name: "Fixture".into(),
        services: vec![discovery::Advertisement::test_from_info(&info)],
    }]
}
#[test]
fn authorization_rechecks_current_approval_identity_and_endpoint_before_socket() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let store = store::Store::new(&config).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let selected = selection(port);
    store.select(&selected).unwrap();
    std::fs::write(&config, "").unwrap();
    assert!(Wire::connect(&config, ID, &selected, &snapshot(port)).is_err());
    std::fs::write(&config, format!("receiver_allowlist=['{ID}']")).unwrap();
    assert!(Wire::connect(&config, "device:112233445566", &selected, &snapshot(port)).is_err());
    assert!(Wire::connect(&config, ID, &selected, &snapshot(port + 1)).is_err());
    let mut changed = snapshot(port);
    changed[0].services[0]
        .capabilities
        .insert("pi".into(), "CHANGED".into());
    assert!(Wire::connect(&config, ID, &selected, &changed).is_err());
    assert!(listener.accept().is_err());
    let wire = Wire::connect(&config, ID, &selected, &snapshot(port)).unwrap();
    drop(wire);
    let (_peer, _) = listener.accept().unwrap();
    std::fs::write(&config, "").unwrap();
    assert!(Wire::connect(&config, ID, &selected, &snapshot(port)).is_err());
    assert!(listener.accept().is_err());
}
#[test]
fn credential_storage_permissions_corruption_identity_and_removal() {
    let dir = tempfile::tempdir().unwrap();
    let store = store::Store::new(&dir.path().join("config.toml")).unwrap();
    assert!(store.credential(ID).unwrap().is_none());
    assert!(!dir.path().join("receiver-state").exists());
    store.save(&credential()).unwrap();
    assert!(store.save(&credential()).is_err());
    assert_eq!(store.credential(ID).unwrap().unwrap().pairing_id, PI);
    let root = dir.path().join("receiver-state");
    let path = root.join("aabbccddeeff.toml");
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(store.credential(ID).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&path, "this is corrupt private data").unwrap();
    assert!(store.credential(ID).is_err());
    store.remove(ID).unwrap();
    store.save(&credential()).unwrap();
    let other = root.join("112233445566.toml");
    std::fs::rename(&path, &other).unwrap();
    assert!(store.credential("device:112233445566").is_err());
    std::os::unix::fs::symlink(&other, &path).unwrap();
    assert!(store.credential(ID).is_err());
    store.remove(ID).unwrap();
    assert!(other.exists());
    assert!(store.remove("../x").is_err());
}

#[test]
fn read_only_dispatch_with_approved_selected_mock_never_connects() {
    use clap::Parser;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, format!("receiver_allowlist=['{ID}']")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let store = store::Store::new(&config).unwrap();
    store.select(&selection(port)).unwrap();
    store.save(&credential()).unwrap();
    for command in [
        vec!["devices"],
        vec!["receivers"],
        vec!["status"],
        vec!["config", "show"],
        vec!["config", "check"],
    ] {
        let mut args = vec!["piair", "--config", config.to_str().unwrap()];
        args.extend(command);
        let cli = crate::cli::Cli::parse_from(args);
        crate::run_with_discovery(cli, |_| Ok(snapshot(port))).unwrap();
        assert!(listener.accept().is_err());
    }
}

#[test]
fn unsupported_profiles_and_ambiguous_identity_are_rejected_before_contact() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, format!("receiver_allowlist=['{ID}']")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let selected = selection(port);
    store::Store::new(&config)
        .unwrap()
        .select(&selected)
        .unwrap();
    for features in ["0x0,0x10000", "0x0,0x84000", "invalid"] {
        let mut observed = snapshot(port);
        observed[0].services[0]
            .capabilities
            .insert("features".into(), features.into());
        assert!(Wire::connect(&config, ID, &selected, &observed).is_err());
    }
    let mut conflicting = snapshot(port);
    let mut impostor = snapshot(port).remove(0);
    impostor.id = "device:112233445566".into();
    conflicting.push(impostor);
    assert!(Wire::connect(&config, ID, &selected, &conflicting).is_err());
    let mut ambiguous = snapshot(port);
    let extra = snapshot(port).remove(0).services.remove(0);
    ambiguous[0].services.push(extra);
    assert!(Wire::connect(&config, ID, &selected, &ambiguous).is_err());
    assert!(listener.accept().is_err());
}

#[test]
fn protocol_failure_closes_socket_without_retry_or_fallback() {
    let (wire, server) = local_mock(|mut socket| {
        request(&mut socket, "/pair-pin-start");
        reply(&mut socket, 1, &[]);
        request(&mut socket, "/pair-setup");
        reply(&mut socket, 2, &[(STATE, &[2]), (7, &[2])]);
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    });
    assert!(pairing::setup(wire, ID, PI, || panic!(
        "PIN must not be requested after rejection"
    ))
    .is_err());
    server.join().unwrap();
}

#[test]
fn credential_cannot_be_used_with_a_different_authorized_receiver() {
    let (wire, server) = local_mock(|mut s| {
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut byte = [0];
        assert_eq!(s.read(&mut byte).unwrap(), 0);
    });
    let mut c = credential();
    c.receiver_id = "device:112233445566".into();
    assert!(pairing::verify(wire, &c).is_err());
    server.join().unwrap();
}
