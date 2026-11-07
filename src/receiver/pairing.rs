use super::{
    crypto::{self, ControlKeys},
    srp,
    store::Credential,
    tlv::*,
    wire::Wire,
};
use anyhow::{ensure, Context, Result};
use ed25519_dalek::{Signer, SigningKey};
use rand::{rngs::OsRng, RngCore};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

fn exchange(
    wire: &mut Wire,
    path: &str,
    fields: &[(u8, &[u8])],
    state: u8,
    allowed: &[u8],
) -> Result<Tlv> {
    let message = Tlv::parse(&wire.post(path, &encode(fields))?)?;
    message.response(state, allowed)?;
    Ok(message)
}
/// Consuming, linear exchange: any error drops the socket and all owned secrets.
/// Credentials are returned only after M6's AEAD AND signed identity verification.
pub fn setup(
    mut wire: Wire,
    receiver_id: &str,
    advertised_pi: Option<&str>,
    pin: impl FnOnce() -> Result<Zeroizing<String>>,
) -> Result<Credential> {
    ensure!(
        wire.receiver_id() == receiver_id,
        "authorized receiver and pairing identity differ"
    );
    setup_inner(&mut wire, receiver_id, advertised_pi, pin)
}
fn setup_inner(
    wire: &mut Wire,
    receiver_id: &str,
    advertised_pi: Option<&str>,
    pin: impl FnOnce() -> Result<Zeroizing<String>>,
) -> Result<Credential> {
    ensure!(
        wire.method() != super::compatibility::Method::Transient,
        "transient method unsupported; no fallback"
    );
    if wire.method().pin_start() {
        let response = wire.post("/pair-pin-start", &[])?;
        ensure!(response.is_empty(), "unsupported PIN-start response");
    }
    let m2 = exchange(
        wire,
        "/pair-setup",
        &[(STATE, &[1]), (0, &[0])],
        2,
        &[SALT, PUBLIC_KEY, FLAGS],
    )?;
    ensure!(m2.integer(FLAGS)?.unwrap_or(0)==0, "receiver proposed transient/split/unknown flags during persistent enrollment; authentication downgrade rejected");
    let pin = pin()?;
    let valid_pin = if wire.method().pin_start() {
        (4..=8).contains(&pin.len()) && pin.bytes().all(|b| b.is_ascii_digit())
    } else {
        pin.len() == 10
            && pin.as_bytes().iter().enumerate().all(|(i, b)| {
                if i == 3 || i == 6 {
                    *b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            })
    };
    ensure!(valid_pin, "invalid setup-code format: displayed PIN is 4..8 digits; existing HAP setup code is XXX-XX-XXX (dashes preserved)");
    wire.restart_deadline();
    let client = srp::Client::start(pin.as_bytes(), m2.get(SALT)?, m2.get(PUBLIC_KEY)?)?;
    drop(pin);
    let m4 = exchange(
        wire,
        "/pair-setup",
        &[
            (STATE, &[3]),
            (PUBLIC_KEY, &client.public),
            (PROOF, client.proof.as_ref()),
        ],
        4,
        &[PROOF],
    )?;
    let key = client.finish(m4.get(PROOF)?)?;
    let encryption = crypto::derive(
        key.as_ref(),
        "Pair-Setup-Encrypt-Salt",
        "Pair-Setup-Encrypt-Info",
    );
    let signing = SigningKey::generate(&mut OsRng);
    let controller_id = uuid::Uuid::new_v4().to_string().to_uppercase();
    let public = signing.verifying_key().to_bytes();
    let x = crypto::derive(
        key.as_ref(),
        "Pair-Setup-Controller-Sign-Salt",
        "Pair-Setup-Controller-Sign-Info",
    );
    let info = Zeroizing::new([x.as_ref(), controller_id.as_bytes(), &public].concat());
    let signature = signing.sign(&info).to_bytes();
    let sub = encode(&[
        (IDENTIFIER, controller_id.as_bytes()),
        (PUBLIC_KEY, &public),
        (SIGNATURE, &signature),
    ]);
    let encrypted = crypto::seal(&encryption, &crypto::nonce(b"PS-Msg05"), &sub, &[])?;
    // M5 can enroll the controller even if M6 is lost or invalid. Never erase
    // remote state automatically, and never retry enrollment on this failure.
    let result = (|| -> Result<Credential> {
        let m6 = exchange(
            wire,
            "/pair-setup",
            &[(STATE, &[5]), (ENCRYPTED, &encrypted)],
            6,
            &[ENCRYPTED],
        )?;
        let plaintext = crypto::open(
            &encryption,
            &crypto::nonce(b"PS-Msg06"),
            m6.get(ENCRYPTED)?,
            &[],
        )?;
        let receiver = Tlv::parse(&plaintext)?;
        receiver.only(&[IDENTIFIER, PUBLIC_KEY, SIGNATURE])?;
        let pairing_id = std::str::from_utf8(receiver.get(IDENTIFIER)?)
            .context("invalid authenticated receiver identifier")?;
        ensure!(
            super::valid_pairing_id(pairing_id),
            "invalid authenticated receiver identifier"
        );
        let receiver_key = receiver.array(PUBLIC_KEY)?;
        let x = crypto::derive(
            key.as_ref(),
            "Pair-Setup-Accessory-Sign-Salt",
            "Pair-Setup-Accessory-Sign-Info",
        );
        let info = Zeroizing::new([x.as_ref(), receiver.get(IDENTIFIER)?, &receiver_key].concat());
        crypto::verify(&receiver_key, &info, receiver.get(SIGNATURE)?)?;
        Ok(Credential {
            version: 2,
            receiver_id: receiver_id.to_owned(),
            pairing_id: pairing_id.to_owned(),
            advertised_pi: advertised_pi.map(str::to_owned),
            controller_id,
            seed: signing.to_bytes(),
            receiver_key,
        })
    })();
    result.context("enrollment may be partial on the receiver; inspect receiver-side controller records before any explicit retry")
}
/// Only verified M4 success transfers the same live wire with directional keys.
/// No socket/secret extraction API is provided; this is not an audio session.
pub fn verify(mut wire: Wire, credential: &Credential) -> Result<Wire> {
    ensure!(
        wire.receiver_id() == credential.receiver_id,
        "credential is bound to a different authorized receiver"
    );
    ensure!(
        wire.method() != super::compatibility::Method::Transient,
        "transient method unsupported; no credential fallback"
    );
    let (keys, events) = verify_inner(&mut wire, credential)?;
    wire.secure(keys, events)
}
fn verify_inner(wire: &mut Wire, credential: &Credential) -> Result<(ControlKeys, ControlKeys)> {
    credential.validate(&credential.receiver_id)?;
    wire.restart_deadline();
    let mut seed = Zeroizing::new([0; 32]);
    OsRng.try_fill_bytes(seed.as_mut())?;
    let private = StaticSecret::from(*seed);
    let public = PublicKey::from(&private).to_bytes();
    let m2 = exchange(
        wire,
        "/pair-verify",
        &[(STATE, &[1]), (PUBLIC_KEY, &public)],
        2,
        &[PUBLIC_KEY, ENCRYPTED],
    )?;
    let receiver_public = m2.array(PUBLIC_KEY)?;
    let shared = private.diffie_hellman(&PublicKey::from(receiver_public));
    ensure!(shared.was_contributory(), "invalid receiver X25519 key");
    let encryption = crypto::derive(
        shared.as_bytes(),
        "Pair-Verify-Encrypt-Salt",
        "Pair-Verify-Encrypt-Info",
    );
    let plaintext = crypto::open(
        &encryption,
        &crypto::nonce(b"PV-Msg02"),
        m2.get(ENCRYPTED)?,
        &[],
    )?;
    let receiver = Tlv::parse(&plaintext)?;
    receiver.only(&[IDENTIFIER, SIGNATURE])?;
    ensure!(
        receiver.get(IDENTIFIER)? == credential.pairing_id.as_bytes(),
        "authenticated receiver identity changed"
    );
    let info = [&receiver_public[..], receiver.get(IDENTIFIER)?, &public].concat();
    crypto::verify(&credential.receiver_key, &info, receiver.get(SIGNATURE)?)?;
    let signing = SigningKey::from_bytes(&credential.seed);
    let info = [
        &public[..],
        credential.controller_id.as_bytes(),
        &receiver_public,
    ]
    .concat();
    let signature = signing.sign(&info).to_bytes();
    let sub = encode(&[
        (IDENTIFIER, credential.controller_id.as_bytes()),
        (SIGNATURE, &signature),
    ]);
    let encrypted = crypto::seal(&encryption, &crypto::nonce(b"PV-Msg03"), &sub, &[])?;
    exchange(
        wire,
        "/pair-verify",
        &[(STATE, &[3]), (ENCRYPTED, &encrypted)],
        4,
        &[],
    )
    .context("pair-verify did not complete")?;
    Ok((
        ControlKeys::new(shared.as_bytes()),
        ControlKeys::events(shared.as_bytes()),
    ))
}
