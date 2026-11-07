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
        version: 2,
        receiver_id: ID.into(),
        pairing_id: PI.into(),
        advertised_pi: Some(PI.into()),
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
    assert!(text.starts_with(&format!("POST {expected} HTTP/1.1\r\n")));
    assert!(text.contains("X-Apple-HKP: 3\r\n"));
    assert!(text.contains("Host: 127.0.0.1:"));
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
    Fields,
    OuterFields,
    M2Fields,
    Type17(usize),
    MalformedType17,
    OuterType17,
    Missing(u8),
}
fn setup_mock(socket: TcpStream, fault: Fault) {
    setup_mock_profile(socket, fault, Method::PinDisplay, "1234", PI)
}
fn setup_mock_profile(
    mut socket: TcpStream,
    fault: Fault,
    method: Method,
    pin: &str,
    signed_id: &str,
) {
    let offset = if method.pin_start() {
        assert!(request(&mut socket, "/pair-pin-start").is_empty());
        reply(&mut socket, 1, &[]);
        1
    } else {
        0
    };
    let m1 = Tlv::parse(&request(&mut socket, "/pair-setup")).unwrap();
    assert_eq!(m1.get(0).unwrap(), [0]);
    let salt = [7; 16];
    let private = [9; 32];
    let verifier =
        SrpClient::<Sha512>::new(&G_3072).compute_verifier(b"Pair-Setup", pin.as_bytes(), &salt);
    let server = SrpServer::<Sha512>::new(&G_3072);
    let b = srp::pad(&server.compute_public_ephemeral(&private, &verifier)).unwrap();
    let mut fields = vec![(STATE, &[2][..]), (SALT, &salt[..]), (PUBLIC_KEY, &b[..])];
    if method == Method::SetupCode {
        fields.push((FLAGS, &[0, 0, 0, 0]));
    }
    if matches!(fault, Fault::M2Fields) {
        fields.push((0, b"synthetic-secret-marker"));
        reply(&mut socket, 1 + offset, &fields);
        return;
    }
    reply(&mut socket, 1 + offset, &fields);
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
        reply(&mut socket, 2 + offset, &[(STATE, &[4]), (7, &[2])]);
        return;
    }
    assert_eq!(m3.get(PROOF).unwrap(), m1.as_ref());
    if matches!(fault, Fault::Proof) {
        reply(&mut socket, 2 + offset, &[(STATE, &[4]), (PROOF, &[0; 64])]);
        return;
    }
    if matches!(fault, Fault::State) {
        reply(
            &mut socket,
            2 + offset,
            &[(STATE, &[6]), (PROOF, m2.as_ref())],
        );
        return;
    }
    reply(
        &mut socket,
        2 + offset,
        &[(STATE, &[4]), (PROOF, m2.as_ref())],
    );
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
        "CHANGED-ID"
    } else {
        signed_id
    };
    let x = derive(
        key.as_ref(),
        "Pair-Setup-Accessory-Sign-Salt",
        "Pair-Setup-Accessory-Sign-Info",
    );
    let signature_id = if matches!(fault, Fault::Identity) {
        signed_id
    } else {
        id
    };
    let info = [x.as_ref(), signature_id.as_bytes(), &public].concat();
    let mut signature = signing.sign(&info).to_bytes();
    if matches!(fault, Fault::Signature) {
        signature[0] ^= 1;
    }
    let mut fields = vec![
        (IDENTIFIER, id.as_bytes()),
        (PUBLIC_KEY, public.as_slice()),
        (SIGNATURE, signature.as_slice()),
    ];
    if matches!(fault, Fault::Fields) {
        fields.push((0, b"synthetic-secret-marker"));
    }
    // Opaque synthetic bytes, not an asserted Apple metadata schema.
    let extension = if let Fault::Type17(length) = fault {
        vec![0xa5; length]
    } else {
        Vec::new()
    };
    if matches!(fault, Fault::Type17(_)) {
        fields.push((17, &extension));
    }
    if let Fault::Missing(kind) = fault {
        fields.retain(|(field, _)| *field != kind);
    }
    let mut sub = encode(&fields);
    if matches!(fault, Fault::MalformedType17) {
        sub.extend_from_slice(&[17, 2, 0xa5]); // truncated value
    }
    let mut encrypted = seal(&encryption, &nonce(b"PS-Msg06"), &sub, &[]).unwrap();
    if matches!(fault, Fault::Tag) {
        encrypted[0] ^= 1;
    }
    let mut outer = vec![(STATE, &[6][..]), (ENCRYPTED, encrypted.as_slice())];
    if matches!(fault, Fault::OuterFields) {
        outer.push((19, b"synthetic-secret-marker"));
    }
    if matches!(fault, Fault::OuterType17) {
        outer.push((17, b"synthetic-secret-marker"));
    }
    reply(&mut socket, 3 + offset, &outer);
    // Setup consumes the wire on error: no retry, cleanup, or playback request.
    let mut byte = [0];
    assert!(matches!(socket.read(&mut byte), Ok(0)));
}
#[test]
fn persistent_pin_setup_verifies_both_proofs_and_identity() {
    let (wire, server) = local_mock(|s| setup_mock(s, Fault::None));
    let c = pairing::setup(wire, ID, Some(PI), || Ok(Zeroizing::new("1234".into()))).unwrap();
    assert_eq!(c.receiver_id, ID);
    assert_eq!(c.receiver_key, credential().receiver_key);
    c.validate(ID).unwrap();
    server.join().unwrap();
}
#[test]
fn setup_failures_identify_stage_and_preserve_partial_enrollment_warning() {
    for (fault, stage, partial) in [
        (Fault::M2Fields, "/pair-setup M2 outer TLV exchange", false),
        (Fault::WrongPin, "/pair-setup M4 outer TLV exchange", false),
        (Fault::Proof, "pair-setup M4 SRP proof verification", false),
        (
            Fault::Signature,
            "pair-setup M6 signature verification",
            true,
        ),
        (
            Fault::Identity,
            "pair-setup M6 signature verification",
            true,
        ),
        (Fault::State, "/pair-setup M4 outer TLV exchange", false),
        (Fault::Tag, "pair-setup M6 decryption", true),
        (Fault::Fields, "pair-setup M6 decrypted payload", true),
        (Fault::Type17(1), "pair-setup M6 decrypted payload", true),
        (Fault::Type17(256), "pair-setup M6 decrypted payload", true),
        (Fault::MalformedType17, "truncated or empty TLV", true),
        (
            Fault::Missing(IDENTIFIER),
            "required pairing field absent",
            true,
        ),
        (
            Fault::Missing(PUBLIC_KEY),
            "required pairing field absent",
            true,
        ),
        (
            Fault::Missing(SIGNATURE),
            "required pairing field absent",
            true,
        ),
        (
            Fault::OuterType17,
            "/pair-setup M6 outer TLV exchange",
            true,
        ),
        (
            Fault::OuterFields,
            "/pair-setup M6 outer TLV exchange",
            true,
        ),
    ] {
        let (wire, server) = local_mock(move |s| setup_mock(s, fault));
        let pin = if matches!(fault, Fault::WrongPin) {
            "9999"
        } else {
            "1234"
        };
        let error = pairing::setup(wire, ID, Some(PI), || Ok(Zeroizing::new(pin.into())))
            .err()
            .expect("setup must fail closed");
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains(stage), "missing fixed stage context");
        assert_eq!(diagnostic.contains("enrollment may be partial"), partial);
        if matches!(fault, Fault::Fields) {
            assert!(diagnostic == "enrollment may be partial on the receiver; inspect receiver-side controller records before any explicit retry: pair-setup M6 decrypted payload: unexpected pairing TLV types: [0]");
        }
        if matches!(fault, Fault::OuterFields) {
            assert!(diagnostic.ends_with("unexpected pairing TLV types: [19]"));
        }
        if let Fault::Type17(length) = fault {
            assert!(diagnostic.contains(&format!(
                "type 17 present; length={length} bytes; semantics unverified"
            )));
            assert!(diagnostic.ends_with("unexpected pairing TLV types: [17]"));
        } else {
            assert!(!diagnostic.contains("semantics unverified"));
            assert!(!diagnostic.contains("length="));
        }
        if matches!(fault, Fault::OuterType17) {
            assert!(diagnostic.ends_with("unexpected pairing TLV types: [17]"));
        }
        // Assertions intentionally never print the diagnostic or fixture values.
        for forbidden in [pin, PI, ID, "CHANGED-ID", "synthetic-secret-marker"] {
            assert!(
                !diagnostic.contains(forbidden),
                "diagnostic exposed fixture data"
            );
        }
        server.join().unwrap();
    }
}
#[test]
fn type17_failure_never_reaches_enrollment_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, format!("receiver_allowlist=['{ID}']")).unwrap();
    let store = store::Store::new(&config).unwrap();
    let selected = selection(7000);
    store.select(&selected).unwrap();
    let (wire, server) = local_mock(|s| setup_mock(s, Fault::Type17(32)));
    let result = pairing::setup(wire, ID, Some(PI), || Ok(Zeroizing::new("1234".into())))
        .and_then(|c| save_verified_enrollment(&config, &selected, &c));
    assert!(result.is_err());
    assert!(store.credential(ID).unwrap().is_none());
    assert!(store.selection().unwrap() == Some(selected));
    server.join().unwrap();
}
fn verify_mock(socket: TcpStream, fault: Fault) {
    verify_mock_with_id(socket, fault, PI)
}
fn verify_mock_with_id(socket: TcpStream, fault: Fault, signed_id: &str) {
    drop(verify_mock_continue(socket, fault, signed_id));
}
fn verify_mock_continue(
    socket: TcpStream,
    fault: Fault,
    signed_id: &str,
) -> Option<(TcpStream, ControlKeys)> {
    verify_mock_before_finish(socket, fault, signed_id, || {})
}
fn verify_mock_before_finish(
    socket: TcpStream,
    fault: Fault,
    signed_id: &str,
    before_m4: impl FnOnce(),
) -> Option<(TcpStream, ControlKeys)> {
    verify_mock_core(socket, fault, signed_id, |_| before_m4())
}
fn verify_mock_core(
    mut socket: TcpStream,
    fault: Fault,
    signed_id: &str,
    before_m4: impl FnOnce(&[u8; 32]),
) -> Option<(TcpStream, ControlKeys)> {
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
        signed_id
    };
    let info = [&public[..], id.as_bytes(), &controller_public].concat();
    let signing = SigningKey::from_bytes(&[22; 32]);
    let mut signature = signing.sign(&info).to_bytes();
    if matches!(fault, Fault::Signature) {
        signature[0] ^= 1;
    }
    let mut fields = vec![
        (IDENTIFIER, id.as_bytes()),
        (SIGNATURE, signature.as_slice()),
    ];
    if matches!(fault, Fault::Fields) {
        fields.push((0, &[0]));
    }
    if matches!(fault, Fault::Type17(_)) {
        fields.push((17, b"synthetic-secret-marker"));
    }
    let sub = encode(&fields);
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
    if matches!(
        fault,
        Fault::Identity | Fault::Signature | Fault::Tag | Fault::Fields | Fault::Type17(_)
    ) {
        return None;
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
    before_m4(shared.as_bytes());
    if matches!(fault, Fault::State) {
        reply(&mut socket, 2, &[(STATE, &[2])]);
        None
    } else {
        reply(&mut socket, 2, &[(STATE, &[4])]);
        let mut keys = ControlKeys::new(shared.as_bytes());
        keys.swap_directions_for_test();
        Some((socket, keys))
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
        Fault::Type17(1),
    ] {
        let (wire, server) = local_mock(move |s| verify_mock(s, fault));
        let result = pairing::verify(wire, &credential());
        assert_eq!(result.is_ok(), matches!(fault, Fault::None));
        if matches!(fault, Fault::Type17(_)) {
            let diagnostic = format!("{:#}", result.err().unwrap());
            assert!(diagnostic.contains("pair-verify M2 decrypted payload"));
            assert!(diagnostic.ends_with("unexpected pairing TLV types: [17]"));
            assert!(!diagnostic.contains("length="));
            assert!(!diagnostic.contains("synthetic-secret-marker"));
        }
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
        advertised_pi: Some(PI.into()),
        method: Method::PinDisplay,
        addresses: ["127.0.0.1".parse().unwrap()].into(),
        port,
        capabilities: Capabilities {
            features: 1 << 46,
            flags: None,
            password: None,
            access_control: None,
            protocol_version: None,
        },
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
    assert!(pairing::setup(wire, ID, Some(PI), || panic!(
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

fn local_mock_method(
    method: Method,
    f: impl FnOnce(TcpStream) + Send + 'static,
) -> (Wire, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let handle = thread::spawn(move || f(listener.accept().unwrap().0));
    (
        Wire::mock_method(socket, Duration::from_secs(5), ID, method),
        handle,
    )
}
#[test]
fn setup_code_profile_skips_pin_start_and_enrolls_actual_signed_identity() {
    for advertised_pi in [Some(PI), None] {
        let (wire, server) = local_mock_method(Method::SetupCode, |s| {
            setup_mock_profile(
                s,
                Fault::None,
                Method::SetupCode,
                "123-45-678",
                "AA:BB:CC:DD:EE:FF",
            )
        });
        let c = pairing::setup(wire, ID, advertised_pi, || {
            Ok(Zeroizing::new("123-45-678".into()))
        })
        .unwrap();
        assert_eq!(c.pairing_id, "AA:BB:CC:DD:EE:FF");
        assert_eq!(c.advertised_pi.as_deref(), advertised_pi);
        let dir = tempfile::tempdir().unwrap();
        let store = store::Store::new(&dir.path().join("config.toml")).unwrap();
        store.save(&c).unwrap();
        assert_eq!(
            store.credential(ID).unwrap().unwrap().pairing_id,
            "AA:BB:CC:DD:EE:FF"
        );
        server.join().unwrap();
    }
}
#[test]
fn setup_code_profile_rejects_incorrect_code_and_bad_proofs_without_persistence() {
    for fault in [
        Fault::WrongPin,
        Fault::Proof,
        Fault::Signature,
        Fault::Identity,
        Fault::State,
        Fault::Tag,
        Fault::Fields,
    ] {
        let (wire, server) = local_mock_method(Method::SetupCode, move |s| {
            setup_mock_profile(s, fault, Method::SetupCode, "123-45-678", PI)
        });
        let code = if matches!(fault, Fault::WrongPin) {
            "999-99-999"
        } else {
            "123-45-678"
        };
        let result = pairing::setup(wire, ID, Some(PI), || Ok(Zeroizing::new(code.into())));
        assert!(result.is_err());
        if matches!(fault, Fault::Signature | Fault::Identity | Fault::Tag) {
            assert!(result.err().unwrap().to_string().contains("partial"));
        }
        server.join().unwrap();
    }
}
#[test]
fn persistent_exchange_rejects_transient_split_and_unknown_flags_before_pin() {
    for flags in [vec![16], vec![16, 0, 0, 1], vec![1], vec![0; 5]] {
        let (wire, server) = local_mock(move |mut s| {
            request(&mut s, "/pair-pin-start");
            reply(&mut s, 1, &[]);
            request(&mut s, "/pair-setup");
            reply(
                &mut s,
                2,
                &[
                    (STATE, &[2]),
                    (SALT, &[7; 16]),
                    (PUBLIC_KEY, &[9; 384]),
                    (FLAGS, &flags),
                ],
            );
            let mut byte = [0];
            assert_eq!(s.read(&mut byte).unwrap(), 0); // no M3 or fallback
        });
        assert!(pairing::setup(wire, ID, Some(PI), || panic!(
            "downgrade must fail before PIN"
        ))
        .is_err());
        server.join().unwrap();
    }
}
#[test]
fn unsupported_transient_never_returns_credentials_or_contacts_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, format!("receiver_allowlist=['{ID}']")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut selected = selection(port);
    selected.method = Method::Transient;
    selected.capabilities.features |= 1 << 48;
    let store = store::Store::new(&config).unwrap();
    store.select(&selected).unwrap();
    let mut observed = snapshot(port);
    observed[0].services[0]
        .capabilities
        .insert("features".into(), "0x0,0x14000".into());
    assert!(Wire::connect(&config, ID, &selected, &observed).is_err());
    assert!(listener.accept().is_err());
    assert!(store.credential(ID).unwrap().is_none());
    // Even a future caller cannot bypass method rejection by directly invoking setup.
    let (wire, server) = local_mock_method(Method::Transient, |mut s| {
        let mut byte = [0];
        assert_eq!(s.read(&mut byte).unwrap(), 0);
    });
    assert!(pairing::setup(wire, ID, None, || panic!("no transient PIN prompt")).is_err());
    server.join().unwrap();
}
#[test]
fn capability_and_method_changes_are_rejected_before_network_contact() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, format!("receiver_allowlist=['{ID}']")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let selected = selection(port);
    let store = store::Store::new(&config).unwrap();
    store.select(&selected).unwrap();
    for (key, value) in [
        ("flags", "0x80"),
        ("pw", "true"),
        ("acl", "2"),
        ("protovers", "99"),
        ("features", "0x0,0x10000"),
    ] {
        let mut observed = snapshot(port);
        observed[0].services[0]
            .capabilities
            .insert(key.into(), value.into());
        assert!(Wire::connect(&config, ID, &selected, &observed).is_err());
    }
    let mut changed = selection(port);
    changed.method = Method::SetupCode;
    store.select(&changed).unwrap();
    assert!(Wire::connect(&config, ID, &selected, &snapshot(port)).is_err());
    assert!(listener.accept().is_err());
}

#[test]
fn actual_signed_id_is_used_for_verify_while_pi_remains_a_discovery_pin() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, format!("receiver_allowlist=['{ID}']")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let store = store::Store::new(&config).unwrap();
    store.select(&selection(port)).unwrap();
    let mut c = credential();
    c.pairing_id = "AA:BB:CC:DD:EE:FF".into();
    store.save(&c).unwrap();
    let server = thread::spawn(move || {
        verify_mock_with_id(
            listener.accept().unwrap().0,
            Fault::None,
            "AA:BB:CC:DD:EE:FF",
        )
    });
    let wire = Wire::connect(&config, ID, &selection(port), &snapshot(port)).unwrap();
    let loaded = store.credential(ID).unwrap().unwrap();
    assert_ne!(
        loaded.advertised_pi.as_deref(),
        Some(loaded.pairing_id.as_str())
    );
    pairing::verify(wire, &loaded).unwrap();
    server.join().unwrap();
}
#[test]
fn approval_revocation_and_reselection_after_verified_setup_prevent_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let store = store::Store::new(&config).unwrap();
    let selected = selection(7000);
    store.select(&selected).unwrap();
    std::fs::write(&config, "").unwrap();
    assert!(save_verified_enrollment(&config, &selected, &credential()).is_err());
    assert!(store.credential(ID).unwrap().is_none());
    std::fs::write(&config, format!("receiver_allowlist=['{ID}']")).unwrap();
    let mut changed = selection(7001);
    changed.method = Method::SetupCode;
    store.select(&changed).unwrap();
    assert!(save_verified_enrollment(&config, &selected, &credential()).is_err());
    assert!(store.credential(ID).unwrap().is_none());
    store.select(&selected).unwrap();
    save_verified_enrollment(&config, &selected, &credential()).unwrap();
}
#[test]
fn malformed_and_conflicting_capabilities_do_not_become_an_implicit_method() {
    for (key, value) in [
        ("pw", "1"),
        ("pw", "False"),
        ("flags", "0x100000000"),
        ("flags", ""),
        ("acl", "-1"),
        ("acl", "garbage"),
        ("protovers", "garbage"),
    ] {
        let mut observed = snapshot(7000);
        observed[0].services[0]
            .capabilities
            .insert(key.into(), value.into());
        assert!(Selection::from_receiver(&observed[0], Method::PinDisplay).is_err());
    }
    let mut observed = snapshot(7000);
    observed[0].services[0]
        .capabilities
        .insert("flags".into(), "0x80".into());
    observed[0].services[0]
        .capabilities
        .insert("sf".into(), "0x0".into());
    assert!(Selection::from_receiver(&observed[0], Method::PinDisplay).is_err());
    observed[0].services[0]
        .capabilities
        .insert("flags".into(), "0x0".into());
    observed[0].services[0]
        .capabilities
        .insert("sf".into(), "0X00".into());
    observed[0].services[0]
        .capabilities
        .insert("ft".into(), "0X0,0X00004000".into());
    assert!(Selection::from_receiver(&observed[0], Method::SetupCode).is_ok());
    observed[0].services[0]
        .capabilities
        .insert("ft".into(), "0x0,0x80000".into());
    assert!(Selection::from_receiver(&observed[0], Method::PinDisplay).is_err());
}
#[test]
fn old_credentials_require_explicit_recovery_and_are_not_silently_reset() {
    let dir = tempfile::tempdir().unwrap();
    let store = store::Store::new(&dir.path().join("config.toml")).unwrap();
    store.save(&credential()).unwrap();
    let path = dir.path().join("receiver-state/aabbccddeeff.toml");
    let mut old = credential();
    old.version = 1;
    std::fs::write(&path, toml::to_string(&old).unwrap()).unwrap();
    let error = store.credential(ID).err().unwrap().to_string();
    assert!(error.contains("unpair"));
    assert!(path.exists());
    store.remove(ID).unwrap();
    assert!(store.credential(ID).unwrap().is_none());
}

#[test]
fn malformed_authenticated_identifiers_are_not_enrolled() {
    let (wire, server) = local_mock_method(Method::SetupCode, |s| {
        setup_mock_profile(
            s,
            Fault::None,
            Method::SetupCode,
            "123-45-678",
            "invalid\nidentity",
        )
    });
    assert!(pairing::setup(wire, ID, None, || Ok(Zeroizing::new("123-45-678".into()))).is_err());
    server.join().unwrap();
}
#[test]
fn extra_mfi_or_method_fields_fail_before_pin_instead_of_negotiating() {
    for kind in [0, 9, ENCRYPTED] {
        let (wire, server) = local_mock(move |mut s| {
            request(&mut s, "/pair-pin-start");
            reply(&mut s, 1, &[]);
            request(&mut s, "/pair-setup");
            reply(
                &mut s,
                2,
                &[
                    (STATE, &[2]),
                    (SALT, &[7; 16]),
                    (PUBLIC_KEY, &[9; 384]),
                    (kind, &[1]),
                ],
            );
            let mut byte = [0];
            assert_eq!(s.read(&mut byte).unwrap(), 0);
        });
        assert!(pairing::setup(wire, ID, None, || panic!(
            "no method negotiation/PIN on unexpected fields"
        ))
        .is_err());
        server.join().unwrap();
    }
}

// Production pair-verify, encoders and state owner; only receiver keys are synthetic.
fn encrypted_request(socket: &mut TcpStream, keys: &mut ControlKeys, seq: u32) {
    let mut raw = Vec::new();
    while !raw.ends_with(b"\r\n\r\n") {
        let mut length = [0; 2];
        socket.read_exact(&mut length).unwrap();
        let size = usize::from(u16::from_le_bytes(length));
        assert!((1..=1024).contains(&size));
        let mut frame = vec![0; size + 18];
        frame[..2].copy_from_slice(&length);
        socket.read_exact(&mut frame[2..]).unwrap();
        raw.extend_from_slice(&keys.decrypt(&frame).unwrap());
    }
    assert_eq!(String::from_utf8(raw).unwrap(), format!("OPTIONS * RTSP/1.0\r\nCSeq: {seq}\r\nUser-Agent: PiAir/0.1\r\nContent-Length: 0\r\n\r\n"));
}
fn encrypted_reply(socket: &mut TcpStream, keys: &mut ControlKeys, response: &[u8]) {
    // Frame boundaries are unrelated to RTSP header boundaries. TCP writes are
    // also split inside the length and authentication tag.
    for chunk in response.chunks(17) {
        for byte in keys.encrypt(chunk).unwrap().iter() {
            socket.write_all(&[*byte]).unwrap();
        }
    }
}
fn assert_eof(socket: &mut TcpStream) {
    let mut byte = [0];
    match socket.read(&mut byte) {
        Ok(0) => (),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => (),
        other => panic!("expected closed connection: {other:?}"),
    }
}
#[test]
fn authenticated_socket_continues_with_independent_counters_and_idempotent_cleanup() {
    use session::{Session, State};
    let (wire, server) = local_mock(|s| {
        let (mut socket, mut keys) = verify_mock_continue(s, Fault::None, PI).unwrap();
        for seq in [3, 4] {
            encrypted_request(&mut socket, &mut keys, seq);
            encrypted_reply(&mut socket, &mut keys, format!("RTSP/1.0 200 OK\r\nCSeq: {seq}\r\nPublic: OPTIONS, SETUP, RECORD, FLUSH, TEARDOWN\r\n\r\n").as_bytes());
        }
        assert_eof(&mut socket);
    });
    let mut session = Session::new();
    assert_eq!(session.state(), State::Disconnected);
    session.start(|| Ok(wire), &credential()).unwrap();
    assert_eq!(session.state(), State::Negotiating);
    // An active owner cannot connect again, even if asked twice.
    assert!(session
        .start(|| panic!("unnecessary reconnect"), &credential())
        .is_err());
    for _ in 0..2 {
        session.check_control().unwrap();
    }
    assert_eq!(session.state(), State::Negotiating); // OPTIONS is not SETUP!
    session.shutdown();
    session.shutdown();
    assert_eq!(session.state(), State::Disconnected);
    assert!(session.failure().is_none());
    server.join().unwrap();
}
#[test]
fn buffered_audio_gate_never_sends_setup_or_media_and_preserves_failure() {
    use session::{Session, State};
    let (wire, server) = local_mock(|s| {
        let (mut socket, _) = verify_mock_continue(s, Fault::None, PI).unwrap();
        assert_eof(&mut socket);
    });
    let mut session = Session::new();
    session.start(|| Ok(wire), &credential()).unwrap();
    assert!(session.start_buffered_audio().is_err());
    assert_eq!(session.state(), State::Failed);
    let reason = session.failure().unwrap().to_owned();
    assert!(reason.contains("FairPlay") && reason.contains("codec"));
    session.shutdown();
    session.shutdown();
    assert!(session.check_control().is_err());
    assert_eq!(session.failure(), Some(reason.as_str()));
    assert!(session
        .start(|| panic!("no reconnect"), &credential())
        .is_err());
    server.join().unwrap();
}
#[test]
fn control_rejects_invalid_status_correlation_lengths_plaintext_and_receiver_termination() {
    for response in [
        "RTSP/1.0 200 OK\r\nCSeq: 2\r\n\r\n", // replayed CSeq
        "RTSP/1.0 200 OK\r\n\r\n",
        "RTSP/1.0 401 Unauthorized\r\nCSeq: 3\r\n\r\n",
        "HTTP/1.1 200 OK\r\nCSeq: 3\r\nContent-Length: 0\r\n\r\n",
        "RTSP/1.0 200 OK\r\nCSeq: 3\r\nContent-Length: 999999\r\n\r\n",
        "RTSP/1.0 200 OK\r\nCSeq: 3\r\nContent-Length: 0\r\ncontent-length: 0\r\n\r\n",
        "RTSP/1.0 200 OK\r\nCSeq: 3\r\nTransfer-Encoding: chunked\r\n\r\n",
        "RTSP/1.0 200 OK\r\nCSeq: 3\r\nConnection: close\r\n\r\n",
        "POST /command RTSP/1.0\r\nCSeq: 3\r\n\r\n", // unsolicited request
        "RTSP/1.0 200 OK\r\nCSeq: 3\r\nContent-Length: 1\r\n\r\nx",
    ] {
        let (wire, server) = local_mock(move |s| {
            let (mut socket, mut keys) = verify_mock_continue(s, Fault::None, PI).unwrap();
            encrypted_request(&mut socket, &mut keys, 3);
            // Single frame allows validating closure without writing after it.
            socket
                .write_all(&keys.encrypt(response.as_bytes()).unwrap())
                .unwrap();
            assert_eof(&mut socket);
        });
        let mut session = session::Session::new();
        session.start(|| Ok(wire), &credential()).unwrap();
        assert!(session.check_control().is_err(), "{response}");
        assert_eq!(session.state(), session::State::Failed);
        server.join().unwrap();
    }
    for mode in 0..5 {
        let (wire, server) = local_mock(move |s| {
            let (mut socket, mut keys) = verify_mock_continue(s, Fault::None, PI).unwrap();
            encrypted_request(&mut socket, &mut keys, 3);
            let mut frame = keys.encrypt(b"RTSP/1.0 200 OK\r\nCSeq: 3\r\n\r\n").unwrap();
            match mode {
                0 => frame[2] ^= 1, // tag no longer valid
                1 => frame = Zeroizing::new(vec![0, 0]),
                2 => frame = Zeroizing::new(vec![1, 4]), // 1025
                3 => frame = Zeroizing::new(b"RTSP/1.0 200 OK\r\n\r\n".to_vec()),
                4 => {
                    frame.pop();
                } // truncated tag
                _ => unreachable!(),
            }
            socket.write_all(&frame).unwrap();
            // An invalid frame can make the client close before this half-close.
            // Linux then returns ENOTCONN; this is already the expected closure.
            if let Err(error) = socket.shutdown(std::net::Shutdown::Write) {
                assert_eq!(error.kind(), std::io::ErrorKind::NotConnected);
            }
            assert_eof(&mut socket);
        });
        let mut session = session::Session::new();
        session.start(|| Ok(wire), &credential()).unwrap();
        assert!(session.check_control().is_err());
        assert!(session.check_control().is_err());
        server.join().unwrap();
    }
}
#[test]
fn authorization_revocation_and_selection_change_close_live_control() {
    for change_selection in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(&config, format!("receiver_allowlist=['{ID}']")).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let selected = selection(port);
        let store = store::Store::new(&config).unwrap();
        store.select(&selected).unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) =
                verify_mock_continue(listener.accept().unwrap().0, Fault::None, PI).unwrap();
            assert_eof(&mut socket); // No OPTIONS after revocation.
        });
        let mut session = session::Session::new();
        session
            .start(
                || Wire::connect(&config, ID, &selected, &snapshot(port)),
                &credential(),
            )
            .unwrap();
        if change_selection {
            store.select(&selection(port + 1)).unwrap();
        } else {
            std::fs::write(&config, "").unwrap();
        }
        assert!(session.check_control().is_err());
        assert_eq!(session.state(), session::State::Failed);
        server.join().unwrap();
    }
}
#[test]
fn session_connect_and_authentication_failures_are_terminal() {
    let mut session = session::Session::new();
    assert!(session
        .start(|| anyhow::bail!("offline"), &credential())
        .is_err());
    assert_eq!(session.state(), session::State::Failed);
    for fault in [Fault::Identity, Fault::Signature, Fault::Tag, Fault::State] {
        let (wire, server) = local_mock(move |s| verify_mock(s, fault));
        let mut session = session::Session::new();
        assert!(session.start(|| Ok(wire), &credential()).is_err());
        assert_eq!(session.state(), session::State::Failed);
        assert!(session.failure().is_some());
        session.shutdown();
        server.join().unwrap();
    }
}

#[test]
fn stalled_control_has_an_absolute_deadline_and_closes_without_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let server = thread::spawn(move || {
        let (mut socket, mut keys) =
            verify_mock_continue(listener.accept().unwrap().0, Fault::None, PI).unwrap();
        encrypted_request(&mut socket, &mut keys, 3);
        // No sleep or response; EOF proves timeout released the live connection.
        assert_eof(&mut socket);
    });
    let wire = Wire::mock(socket, Duration::from_millis(300), ID);
    let mut session = session::Session::new();
    session.start(|| Ok(wire), &credential()).unwrap();
    assert!(session.check_control().is_err());
    assert_eq!(session.state(), session::State::Failed);
    assert!(session.failure().unwrap().contains("timed out"));
    server.join().unwrap();
}

#[test]
fn approval_revoked_during_verify_prevents_live_session_handoff() {
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
    let changed = config.clone();
    let server = thread::spawn(move || {
        let (mut socket, _) =
            verify_mock_before_finish(listener.accept().unwrap().0, Fault::None, PI, || {
                std::fs::write(changed, "").unwrap();
            })
            .unwrap();
        assert_eof(&mut socket);
    });
    let mut session = session::Session::new();
    assert!(session
        .start(
            || Wire::connect(&config, ID, &selected, &snapshot(port)),
            &credential()
        )
        .is_err());
    assert_eq!(session.state(), session::State::Failed);
    assert!(session.failure().unwrap().contains("revoked"));
    server.join().unwrap();
}
#[test]
fn replayed_encrypted_response_and_plaintext_reentry_close_control() {
    let (wire, server) = local_mock(|s| {
        let (mut socket, mut keys) = verify_mock_continue(s, Fault::None, PI).unwrap();
        encrypted_request(&mut socket, &mut keys, 3);
        let frame = keys.encrypt(b"RTSP/1.0 200 OK\r\nCSeq: 3\r\n\r\n").unwrap();
        socket.write_all(&frame).unwrap();
        encrypted_request(&mut socket, &mut keys, 4);
        socket.write_all(&frame).unwrap();
        assert_eof(&mut socket);
    });
    let mut session = session::Session::new();
    session.start(|| Ok(wire), &credential()).unwrap();
    session.check_control().unwrap();
    assert!(session.check_control().is_err());
    assert!(session
        .failure()
        .unwrap()
        .contains("authentication tag invalid"));
    server.join().unwrap();

    let (wire, server) = local_mock(|s| {
        let (mut socket, _) = verify_mock_continue(s, Fault::None, PI).unwrap();
        assert_eof(&mut socket); // No plaintext POST after upgrade.
    });
    let mut control = pairing::verify(wire, &credential()).unwrap();
    assert!(control.post("/pair-verify", &[]).is_err());
    assert!(control.options().is_err());
    control.close();
    control.close();
    server.join().unwrap();
}

#[path = "stream_tests.rs"]
mod stream_tests;
