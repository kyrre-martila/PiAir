# Receiver authentication: current evidence and safe verification

Pairing means permanent, verified credential enrollment. Authentication establishes
a session. Transient authentication uses session-scoped SRP without persistent
receiver identity verification; it never makes a receiver permanently paired.
Playback requires separate protocol negotiation and physical verification.
Successful SRP, encrypted control or RTP transmission does not prove audible audio.

## Implementation audit (main after PR #31)

Audited `interactive.rs`, `compatibility.rs`, `pairing.rs`, `wire.rs`, `session.rs`,
`receiver/mod.rs`, `cli.rs`, the pairing/transient/OwnTone/diagnostics/control
verification documents, and receiver/CLI tests. The table describes implemented
paths, independent fixtures/loopback mocks, and operator-reported hardware results.
No physical receiver was contacted for this audit.

| Path | Implemented and mock-tested behavior | Physical evidence and limits |
| --- | --- | --- |
| Persistent PIN-display | HKP 3 PIN-start, method 0 M1–M6; verify SRP proof, M6 AEAD and signed identity before saving credentials | Apple TV enrollment reported successful in [PR #21 evidence](INITIAL_SETUP_DIAGNOSTICS.md). Requires supported PIN display or equivalent independently verified PIN delivery. Earlier HomePod PIN-start 403 does not establish incompatibility for every HomePod/firmware. |
| Existing HAP setup code | Same persistent enrollment without PIN-start; genuine independently obtained `XXX-XX-XXX` code required | Mock-tested; no successful HomePod enrollment or usable code documented. AirPlay passwords are different and password-protected authentication is unsupported. Never invent codes. |
| Legacy transient | Explicit `transient` selection; HTTP HKP 4 M1–M4, verified receiver SRP proof, same-socket control upgrade; no M5/M6 or pair-verify | Earlier HomePod M1 403; no successful legacy profile hardware result established. |
| OwnTone-reference transient | Explicit authenticate-only flag; RTSP `/info`, policy validation, HKP 4 M1–M4, verified SRP proof and same-socket encryption | Later HomePod proof/channel success reported; OPTIONS initially encrypted 403, then reported successful after #20. No confirmed audible playback. |
| pyatv diagnostic | Explicit authenticate-only flag; HTTP HKP 4 PIN-start preamble and pyatv M1/M3 envelope/order; still verifies M4 proof | Earlier preamble 200 followed by M1 403/400; no successful PiAir authentication documented for this profile. |
| Persistent pair-verify | Stored signed receiver ID/key, fresh X25519, verified signature/AEAD, mutual exchange M1–M4; no credential replacement | Apple TV pair-verify success reported in PR #21 evidence. Not proof of HomePod persistent compatibility. |
| Encrypted control check | One encrypted OPTIONS on authenticated socket; validate AEAD/framing, status, matching CSeq and response shape | HomePod encrypted 403 proves a channel but failed OPTIONS acceptance. Later post-#20 success is operator-reported, not a new workspace reproduction. |
| Stream-test | Persistent pair-verify or explicitly selected legacy transient; separate GET/info, SETUP/RECORD and media negotiation | OwnTone/pyatv authenticate flags are unavailable here. Later Apple TV transport observations do not establish HomePod playback or audible output. |

`Capabilities::decide` checks the explicitly requested method, never searches for
another. Persistent paths require bit 46; bit 51 alone is insufficient but is not
a blanket rejection when bit 46 is present. Transient requires bits 43/48 and
unprotected status. Password, unresolved Home/ACL policy, unknown versions and
changed pinned security metadata fail closed. Advertisements are untrusted policy
inputs, not proof of method prerequisites or compatibility. No friendly name,
model, address or advertisement chooses an authentication method automatically.

The wizard keeps enrollment separate from transient authentication. It requires
explicit prerequisite confirmation, even for an already selected persistent
method, before final operation authorization. Missing prerequisites, q, EOF,
invalid input or default-No cancellation preserve configuration, active selection
and credentials. Existing paired receivers are directed to explicit selection and
authentication, with no replacement or automatic network operation. Selecting a
new receiver/method remains provisional until final confirmation; PR #30's
post-confirmation revalidation and concurrent-writer protection remain intact.

Experimental transient profiles remain explicit commands. Adding a profile-aware
wizard would extend orchestration and provisional selection semantics; this PR
provides guidance to the existing command instead. There is no profile cycling,
protocol fallback or automatic extension of stream-test authentication.

## Even-Olai HomePod evidence, in order

Exact independently verified receiver: `device:3ae5c9a302ad` (AudioAccessory5,1,
reported firmware 27.2). Earlier [diagnostics](AUTHENTICATION_DIAGNOSTICS.md) and
[pyatv experiments](PYATV_TRANSIENT_PREAMBLE.md) record HTTP 403/400; the initial
OwnTone-reference RTSP `/info` also returned 403. The [User-Agent investigation](HOMEPOD_INFO_403.md)
identified a concrete reference mismatch. Later [encrypted control evidence](ENCRYPTED_CONTROL_CHECK.md)
records `/info`, M2 and M4 status 200, verified receiver proof and established
control encryption, followed by a valid encrypted OPTIONS 403. Finally,
[PR #21's operator report](INITIAL_SETUP_DIAGNOSTICS.md) explicitly records HomePod
transient SRP and encrypted OPTIONS success after #20. This later report supersedes
older blanket statements that HomePod authentication/control are unverified; it
contains no new raw trace and must not be presented as a locally reproduced result.

Unresolved: reproducibility on the current receiver/policy/firmware, precise
causes of earlier rejections, availability/acceptance of a genuine persistent
enrollment prerequisite, Home authorization/password/MFi requirements where
applicable, subsequent playback negotiation, and physically confirmed audible
output. No general HomePod compatibility or speculative protocol fix follows.

## Next Raspberry Pi diagnostic: authentication and control only

1. Obtain explicit owner/operator authorization for **only** Even-Olai HomePod,
   `device:3ae5c9a302ad`, and this single OwnTone-reference authentication/control
   attempt. Do not contact Stua Apple TV. Do not change receiver access policy,
   production OwnTone/autostream services, persistent enrollment or credentials.
2. Build this reviewed revision in a separate checkout with
   `cargo build --release --locked`; record `git rev-parse HEAD`. Use its standalone
   `target/release/piair` and your existing absolute config path throughout.
3. Run `piair --config /existing/config.toml receivers` for passive discovery.
   Independently verify the exact ID and current endpoint with the owner. Confirm
   that only the intended ID is authorized for this test in the existing
   `receiver_allowlist`; if missing, manually approve only that ID, then run
   `piair --config /existing/config.toml config check`. Stop if identity is uncertain.
4. Inspect the existing active selection. If it is already this ID with method
   `transient` and the correct pinned endpoint, retain it. Otherwise, only after
   explicit approval to change the active selection, run:
   `piair --config /existing/config.toml select device:3ae5c9a302ad --method transient`.
   This deliberately changes selection but neither enrolls nor replaces credentials.
   Cancellation of the subsequent authenticate command leaves that selection as is.
5. In an interactive terminal run exactly one attempt:

   ```sh
   piair --config /existing/config.toml -v authenticate device:3ae5c9a302ad --experimental-owntone-transient --control-check
   ```

   Review the displayed exact ID and explicitly confirm `y`/`yes` only if authorized.
   Default No/EOF cancels before connecting. Transient SRP does not verify a
   persistent receiver identity. Do not run `pair`, `unpair`, `pairing`, stream-test,
   SETUP experiments or playback in this round. Do not try alternate profiles.
6. Record commit, profile, advertised model/firmware (missing stays missing),
   phase, numeric response status and outcome: `/info`, M1/M2, M3/M4, proof/channel
   success or error category, then encrypted OPTIONS acceptance/rejection and
   final command exit outcome. Stop on any rejection, malformed response, bad
   proof, timeout or closure. A 403 is a failed diagnostic, not permission to retry
   or proceed to audio. The command closes the connection without audio setup.
7. Share only this reviewed phase/status summary. Do not log or share credentials,
   PINs, session keys, raw cryptographic payloads or packet captures. Existing `-v`
   diagnostics omit those values but contain local device metadata; review before
   sharing. The OwnTone profile may initialize its existing public sender identity
   file; it does not create receiver pairing credentials.
