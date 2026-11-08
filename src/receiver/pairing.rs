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
    wire.auth_phase(match (path, state) {
        ("/pair-setup", 2) => "pair-setup M1/M2",
        ("/pair-setup", 4) => "pair-setup M3/M4",
        ("/pair-setup", 6) => "pair-setup M5/M6",
        ("/pair-verify", 2) => "pair-verify M1/M2",
        ("/pair-verify", 4) => "pair-verify M3/M4",
        _ => "authentication exchange",
    });
    (|| -> Result<Tlv> {
        let message = Tlv::parse(&wire.post(path, &encode(fields))?)?;
        message.response(state, allowed)?;
        Ok(message)
    })()
    .with_context(|| format!("{path} M{state} outer TLV exchange"))
}
// No plaintext or field values enter diagnostic context.
fn decrypted_payload(plaintext: &[u8], allowed: &[u8]) -> Result<Tlv> {
    let payload = Tlv::parse(plaintext)?;
    payload.only(allowed)?;
    Ok(payload)
}
// Apple-specific type 0x11 is optional opaque metadata only in decrypted M6.
// Do not interpret its contents or use them as identity/authentication evidence.
// Required fields and the full Ed25519 transcript are verified by setup_inner.
fn setup_m6_payload(plaintext: &[u8]) -> Result<Tlv> {
    decrypted_payload(plaintext, &[IDENTIFIER, PUBLIC_KEY, SIGNATURE, 0x11])
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
        wire.auth_phase("PIN-start");
        let response = wire
            .post("/pair-pin-start", &[])
            .context("pair-setup PIN-start exchange")?;
        ensure!(response.is_empty(), "unsupported PIN-start response");
    }
    let m2 = exchange(
        wire,
        "/pair-setup",
        &[(STATE, &[1]), (0, &[0])],
        2,
        &[SALT, PUBLIC_KEY, FLAGS],
    )?;
    ensure!(m2.integer(FLAGS).context("pair-setup M2 flags")?.unwrap_or(0)==0, "pair-setup M2: receiver proposed transient/split/unknown flags during persistent enrollment; authentication downgrade rejected");
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
    let client = (|| srp::Client::start(pin.as_bytes(), m2.get(SALT)?, m2.get(PUBLIC_KEY)?))()
        .context("pair-setup M2 SRP parameters")?;
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
    let key =
        (|| client.finish(m4.get(PROOF)?))().context("pair-setup M4 SRP proof verification")?;
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
    let encrypted = crypto::seal(&encryption, &crypto::nonce(b"PS-Msg05"), &sub, &[])
        .context("pair-setup M5 encryption")?;
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
            m6.get(ENCRYPTED)
                .context("pair-setup M6 outer TLV fields")?,
            &[],
        )
        .context("pair-setup M6 decryption")?;
        let receiver = setup_m6_payload(&plaintext).context("pair-setup M6 decrypted payload")?;
        let credential = (|| -> Result<Credential> {
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
            let info =
                Zeroizing::new([x.as_ref(), receiver.get(IDENTIFIER)?, &receiver_key].concat());
            crypto::verify(&receiver_key, &info, receiver.get(SIGNATURE)?)
                .context("pair-setup M6 signature verification")?;
            Ok(Credential {
                version: 2,
                receiver_id: receiver_id.to_owned(),
                pairing_id: pairing_id.to_owned(),
                advertised_pi: advertised_pi.map(str::to_owned),
                controller_id,
                seed: signing.to_bytes(),
                receiver_key,
            })
        })()
        .context("pair-setup M6 authenticated identity")?;
        Ok(credential)
    })();
    result.context("enrollment may be partial on the receiver; inspect receiver-side controller records before any explicit retry")
}
/// Transient SRP authenticates this connection, not a persistent receiver identity.
/// No enrollment M5/M6, credential object, or separate pair-verify is involved.
pub fn transient(mut wire: Wire) -> Result<Wire> {
    ensure!(
        wire.method() == super::compatibility::Method::Transient,
        "explicit transient selection required; no fallback"
    );
    wire.restart_deadline();
    // Match pinned pyatv's insertion order only in the explicit experiment.
    let m1: [(u8, &[u8]); 3] = if wire.uses_pyatv_transient_profile() {
        [(0, &[0]), (STATE, &[1]), (FLAGS, &[0x10])]
    } else {
        [(STATE, &[1]), (0, &[0]), (FLAGS, &[0x10])]
    };
    let m2 = exchange(&mut wire, "/pair-setup", &m1, 2, &[SALT, PUBLIC_KEY, FLAGS])
        .context("transient authentication M2")?;
    ensure!(
        m2.integer(FLAGS)?.is_none_or(|flags| flags == 0x10),
        "transient M2: unsupported authentication flags"
    );
    if wire.uses_owntone_transient_profile() {
        ensure!(
            m2.get(SALT)?.len() == 16,
            "OwnTone transient M2 requires 16-byte salt"
        );
    }
    let client = srp::Client::transient(m2.get(SALT)?, m2.get(PUBLIC_KEY)?)
        .context("transient SRP parameters")?;
    // The reference encodes A as a minimal unsigned integer. SRP calculations
    // still use group-width padding where required by the existing profile.
    let public = if wire.uses_owntone_transient_profile() {
        &client.public[client
            .public
            .iter()
            .position(|b| *b != 0)
            .context("zero SRP public key")?..]
    } else {
        &client.public[..]
    };
    let m4 = exchange(
        &mut wire,
        "/pair-setup",
        &[
            (STATE, &[3]),
            (PUBLIC_KEY, public),
            (PROOF, client.proof.as_ref()),
        ],
        4,
        &[PROOF],
    )
    .context("transient authentication M4")?;
    let shared = client
        .finish(m4.get(PROOF)?)
        .context("transient cryptographic receiver proof verification failed")?;
    let mut wire = wire
        .secure(
            ControlKeys::new(shared.as_ref()),
            ControlKeys::events(shared.as_ref()),
        )
        .context("transient encrypted channel establishment failed")?;
    wire.install_audio_key(shared.as_ref())?;
    Ok(wire)
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
    let (keys, events, shared) = verify_inner(&mut wire, credential)?;
    let mut wire = wire.secure(keys, events)?;
    wire.install_audio_key(shared.as_ref())?;
    Ok(wire)
}
fn verify_inner(
    wire: &mut Wire,
    credential: &Credential,
) -> Result<(ControlKeys, ControlKeys, Zeroizing<[u8; 32]>)> {
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
    let receiver_public = m2
        .array(PUBLIC_KEY)
        .context("pair-verify M2 outer TLV fields")?;
    let shared = private.diffie_hellman(&PublicKey::from(receiver_public));
    ensure!(
        shared.was_contributory(),
        "pair-verify M2: invalid receiver X25519 key"
    );
    let encryption = crypto::derive(
        shared.as_bytes(),
        "Pair-Verify-Encrypt-Salt",
        "Pair-Verify-Encrypt-Info",
    );
    let plaintext = crypto::open(
        &encryption,
        &crypto::nonce(b"PV-Msg02"),
        m2.get(ENCRYPTED)
            .context("pair-verify M2 outer TLV fields")?,
        &[],
    )
    .context("pair-verify M2 decryption")?;
    let receiver = decrypted_payload(&plaintext, &[IDENTIFIER, SIGNATURE])
        .context("pair-verify M2 decrypted payload")?;
    ensure!(
        receiver.get(IDENTIFIER)? == credential.pairing_id.as_bytes(),
        "pair-verify M2: authenticated receiver identity changed"
    );
    let info = [&receiver_public[..], receiver.get(IDENTIFIER)?, &public].concat();
    crypto::verify(&credential.receiver_key, &info, receiver.get(SIGNATURE)?)
        .context("pair-verify M2 signature verification")?;
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
        Zeroizing::new(*shared.as_bytes()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m6_opaque_type17_still_requires_well_formed_bounded_tlv() {
        // These are structural TLV failures, not proposed metadata format rules.
        for malformed in [
            vec![17],
            vec![17, 0],
            vec![17, 2, 0xa5],
            vec![17, 1, 0xa5, 17, 1, 0xa5],
            vec![0; LIMIT + 1],
        ] {
            let diagnostic = format!("{:#}", setup_m6_payload(&malformed).err().unwrap());
            assert!(!diagnostic.contains("length="));
            assert!(!diagnostic.contains("semantics unverified"));
        }
        let encoded = encode(&[(17, b"synthetic-secret-marker"), (18, b"private")]);
        let diagnostic = format!("{:#}", setup_m6_payload(&encoded).err().unwrap());
        assert!(!diagnostic.contains("length="));
        assert!(diagnostic.ends_with("unexpected pairing TLV types: [18]"));
        assert!(!diagnostic.contains("synthetic-secret-marker"));
        assert!(!diagnostic.contains("private"));
    }
}
