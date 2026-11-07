# Receiver identity and authentication foundation

This is independently written protocol composition, not an AirPlay sender library.
No source from OwnTone, Autostream, airplay2-rs, or any other AirPlay sender was
copied, translated or incorporated. Research used protocol documentation and
published numerical test vectors; discovery and audio code were inspected before
implementation. No real receiver was contacted during development.

## Research and evidence

Sources consulted 2026-11-07:

- [Apple HomeKit Accessory Protocol specification, non-commercial R2,
  2019-07-26 (public mirror)](https://forum.iobroker.net/assets/uploads/files/1634848447889-apple-spezifikation-homekit.pdf),
  sections 5.5–5.10: SRP profile/test vectors, setup, verify, TLV8 and session
  security. This describes HAP, **not a universal AirPlay interoperability promise**.
- [Independently documented AirPlay discovery](https://openairplay.github.io/airplay-spec/service_discovery.html):
  device ID, low/high feature words, pairing identifier `pi`, and authentication
  advertisements. [Feature assignments](https://openairplay.github.io/airplay-spec/features.html)
  distinguish buffered audio, PTP, system/HK/transient pairing and MFi.
- [HomeKit-based AirPlay pairing and control encryption](https://openairplay.github.io/airplay-spec/pairing/hkp.html):
  distinguishes persistent and transient paths. This is an incomplete unofficial
  description; it does not establish firmware-specific endpoint/header behavior.
- [Apple Platform Security overview](https://help.apple.com/pdf/security/en_US/apple-platform-security-guide.pdf):
  cryptographic background, not an AirPlay endpoint specification.
- [RFC 5054](https://www.rfc-editor.org/rfc/rfc5054) (3072-bit group),
  [RFC 5869](https://www.rfc-editor.org/rfc/rfc5869) (HKDF),
  [RFC 8439](https://www.rfc-editor.org/rfc/rfc8439) (ChaCha20-Poly1305),
  [RFC 7748](https://www.rfc-editor.org/rfc/rfc7748) (X25519),
  [RFC 8032](https://www.rfc-editor.org/rfc/rfc8032) (Ed25519).

The HAP numerical fixture in `tests/fixtures/hap-srp.toml` contains test data from
section 5.5.2, not implementation source. It checks public A and session K against
Apple's values independently of the mock. The `srp` crate's convenience proof is
not HAP-compatible: PiAir uses its arithmetic API, padded A/B scrambling parameter,
SHA-512(S), and the HAP identity-bound proof composition. SHA-512, HKDF, signatures,
DH, AEAD, random generation and constant-time comparison use established crates.
No custom hash, cipher, curve or modular exponentiation primitive is implemented.

## Implemented profile and exact gaps

Implemented: one deliberately narrow persistent HAP profile: explicitly requested
PIN display, method 0 (Pair Setup), SRP-6a 3072/SHA-512 M1–M4, AEAD and mutually
signed identity exchange M5–M6, then a separately invoked X25519/Ed25519 pair-verify
M1–M4. Control read/write key derivation and 1..1024-byte authenticated framing
are implemented/tested but not connected to an application request transport.
No audio session, GET /info, SETUP, RECORD, media keys, timing or streaming exists.

The selected profile requires an `_airplay._tcp` advertisement with well-formed
features, HK pairing/access-control bit 46, no MFi bit 51, and a printable `pi`.
Bit 40 is reported only as an **advertised buffered-audio capability**. Model names,
AirPlay version strings and feature flags never establish verified compatibility.

The wire profile sends POSTs using RTSP/1.0, CSeq, `X-Apple-HKP: 3`, and
`application/octet-stream`. It accepts bounded HTTP/1.1 or RTSP/1.0 200 responses
with explicit Content-Length (RTSP requires matching CSeq). These AirPlay-specific
header/PIN-start choices are an interoperability assumption pending hardware
captures; local mocks verify this profile, not that Apple firmware accepts it.
Unexpected TLV fields, alternate methods and protocol variants are rejected,
including a receiver attempting to add MFi material during setup.

| Intended receiver path | Evidence and remaining gap |
| --- | --- |
| Apple TV advertising persistent HK pairing | Candidate for this PIN profile. Requires controlled tests of `/pair-pin-start`, HKP header, method 0, framing and signed pairing ID equal to selected `pi`. Some firmware may use a MAC identifier instead, require MFi/method 1, or implement legacy device verification. Those variants fail closed. No Apple TV model or tvOS version is verified. |
| HomePod advertising persistent HK pairing and providing a usable PIN | Same candidate profile, unverified. HomePod has no screen; PiAir cannot assume that PIN display or this enrollment path is available. |
| HomePod exposing only system/transient pairing | Unsupported. Transient setup ends after SRP M4, commonly using a fixed code; it does not exchange durable signed receiver identity/credentials. Treating its shared secret as persistent authenticated identity would violate this milestone's identity binding. A separate explicitly authorized transient session design, firmware evidence and accurate trust semantics are needed. No fixed PIN is silently attempted. |
| MFi, legacy Apple TV verification, FairPlay, RSA/Digest/password-only auth | Unsupported. No MFi certificate-chain validation, legacy plist/SRP profile, `/fp-setup` or `/auth-setup` implementation. No downgrade or unauthenticated fallback. |

Split pairing, TLV transport fragmentation/lists, group/stereo HomePod enrollment,
pairing administration and encrypted control I/O are also unimplemented. The
precise gap is firmware-specific method negotiation and identity mapping, not
missing placeholder success responses. Do not infer these variants from a display
name or keep trying endpoints until something responds.

## Approval and identity rules

1. `devices`/`receivers` perform only bounded passive DNS-SD browsing. The Samsung
   TV and all other unapproved devices receive **no direct connection**. No active
   receiver probing is performed by any discovery/status/config/audio command.
2. Independently verify the physical receiver and its canonical
   `device:<12 lowercase hex digits>`, then explicitly edit `receiver_allowlist`.
   An empty list denies everything. No CLI name/IP/hostname substitution exists.
3. `select <id>` requires that allowlist entry. It pins the exact discovered
   address set, port, pairing ID and feature bits in local selection state.
   It does not connect, authenticate, play, or grant allowlist approval.
4. Pair/authenticate require an interactive terminal and typing the exact selected
   ID. Pair additionally prompts for a hidden PIN from the physical receiver.
   EOF/piped input cannot approve pairing. PINs are never accepted as CLI arguments.
5. Immediately before a socket, `Wire::connect` reloads the exact config file,
   verifies current approval and stored selection, compares the fresh passive
   snapshot, and rejects conflicting identities sharing an address. It accepts
   literal pinned addresses only; there is **no hostname/DNS resolution path**.
6. Choose a deterministic IPv4 address when available, otherwise the first IPv6
   address. Make one connection attempt. No address fallback, redirects or retries.
   IPv6 link-local addresses needing a scope ID are currently unsupported.
7. PIN-authenticated setup verifies the signed identifier equals the selected
   pairing ID before returning credentials. Pair-verify uses the stored receiver
   public key and exact pairing ID. A new key/identity cannot silently replace it.

Advertisements remain untrusted. Approval is user intent; PIN enrollment is the
initial cryptographic binding, not a manufacturer attestation. A spoofed ad can
cause denial of service or direct the **user-approved** enrollment endpoint to an
attacker, but cannot finish without the PIN proof and matching signed ID. Verify
the physical PIN source. A repeated forged stable ID on another endpoint is
rejected when ambiguous; an endpoint change requires explicit reselection.

Current approval is checked before **each** connection, including authenticate.
There are no background retries/reconnections. Revocation is observed at connection
and before saving enrollment, not by a background watcher; an already open short
handshake is not instantly terminated by editing TOML. Future streaming must own
an authorized `Wire`, add live session revocation policy as needed, and never open
an arbitrary receiver socket. The private socket cannot be extracted from Wire.

## Sequences and state

```mermaid
sequenceDiagram
    actor User
    participant CLI
    participant Boundary as Approval boundary
    participant R as Selected receiver
    User->>CLI: allowlist exact ID; select ID (passive only)
    User->>CLI: pair ID; type exact ID
    CLI->>Boundary: current config + selection + fresh snapshot
    Boundary->>R: one bounded TCP connect
    CLI->>R: POST /pair-pin-start
    CLI->>R: /pair-setup M1 (method 0)
    R-->>CLI: M2 salt + SRP public B
    User->>CLI: hidden physical PIN
    CLI->>R: M3 public A + client proof
    R-->>CLI: M4 server proof
    Note over CLI: verify SRP proof
    CLI->>R: M5 encrypted controller ID/key/signature
    R-->>CLI: M6 encrypted receiver ID/key/signature
    Note over CLI: verify AEAD, expected ID, Ed25519 signature
    CLI->>CLI: recheck approval; atomically save credentials
    Note over CLI,R: socket closes; no audio session
```

```mermaid
sequenceDiagram
    actor User
    participant CLI
    participant Boundary as Approval boundary
    participant R as Selected receiver
    User->>CLI: authenticate ID; type exact ID
    CLI->>Boundary: reload approval, selection and passive snapshot
    Boundary->>R: one bounded TCP connect
    CLI->>R: /pair-verify M1 fresh X25519 public key
    R-->>CLI: M2 ephemeral key + AEAD(receiver ID/signature)
    Note over CLI: reject zero DH; verify stored ID/key and signature
    CLI->>R: M3 AEAD(controller ID/signature)
    R-->>CLI: M4 success
    Note over CLI: authenticated exchange succeeded; derive control keys
    Note over CLI,R: socket closes and session keys drop; no playback
```

`discovered`, `approved`, `selected` and `paired` are independent local facts.
`paired` means validated stored enrollment material, not current liveness or
successful authentication. `receivers` always reports authenticated=false and
connected=false for that invocation. `authenticate` reports success only after
pair-verify M4 and immediately closes. There is no retained connected state.

Every I/O operation has a 3-second limit and the automated handshake has a
15-second deadline per automated phase, recalculated between partial reads/writes to defeat trickle
traffic. Interactive user think time is excluded; the deadline restarts after PIN
entry. Connect has a 3-second timeout. State functions execute linearly, reject
wrong state/error TLVs, and own the socket, closing it on success and failure.
Messages/headers cap at 8 KiB. Empty/duplicate/invalid TLV fields, invalid SRP and
X25519 keys, bad signatures/tags and unsupported response framing fail closed.
Control framing poisons and wipes its keys on any failure; no reuse after errors.

Owned PINs, keys, proofs, TLVs, decrypted messages and serialization buffers use
zeroizing storage; dalek secret types wipe on drop. **No complete process-memory
scrubbing guarantee** is claimed: serde/TOML and SRP big-integer dependency
intermediates/allocators may retain copies, and num-bigint arithmetic is not
constant-time. This is not hardened against a hostile local process/timing
observer. SRP proof/tag/signature verification uses established constant-time or
strict library verification. There are no secret Debug implementations, raw
payload logs, or credential-printing commands.

## Credential storage and recovery

Storage is `receiver-state/` next to the resolved config path. The parent config
directory must exist before writes. Selection is `selected.toml`; each credential
is `<12hex>.toml`. This is a separate, versioned secret format, not ordinary
configuration. Paths are derived only from validated IDs. Multiple config files
in one directory intentionally share receiver state; use separate directories for
independent controllers.

Linux requires owned nonsymlink directory mode 0700, regular single-link owned
files mode 0600, O_NOFOLLOW opens, strict parsing, validated key/ID/version and a
16 KiB bound. Same-directory temporary files are synced; credential creation uses
no-clobber persistence, while selection uses atomic replacement. Corrupt or
permissive state is rejected, never used for authentication or silently reset.
Files and directory are synced after saves/removal. Parent directories are trusted
local configuration paths; a malicious process running as the same user can replace
state and is outside this filesystem-permission protection model.

**Filesystem permissions are the primary local protection. There is no encryption
at rest or OS keyring.** Protect backups and do not commit receiver-state. Unpair
removes only the local credential, even when corrupt; it does not revoke remote
controller records, clear selection, or remove allowlist approval. Also remove the
allowlist entry to prohibit future contact. Inspect hardware changes before
unpair/re-enroll. A receiver may retain its controller record if its M6 was sent
but local verification/write failed; remove that stale pairing on the receiver.
A full receiver pairing table may require receiver-side reset/removal.

## Controlled commands (future hardware validation)

Use only an independently identified receiver you intend to contact:

```sh
piair --config config.toml receivers
# Edit receiver_allowlist in config.toml with the exact verified ID; never a name.
piair --config config.toml select device:aabbccddeeff
piair --config config.toml pair device:aabbccddeeff
piair --config config.toml authenticate device:aabbccddeeff
piair --config config.toml unpair device:aabbccddeeff
```

The ID above is a documentation example, not authorization for any device.
Record model/firmware, advertised capabilities and sanitized error/state only.
Do not save PINs, credential files, authentication bodies or session keys in test
reports. Stop on unsupported behavior; no repeated probes or downgrades. No
Samsung/unapproved receiver testing, playback, service installation or changes to
Autostream/OwnTone are part of this milestone.

## Local validation

Run `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, and
`cargo test --locked`. Mocks use loopback, fixed receiver-side key material and
synthetic discovery snapshots; client cryptographic randomness stays OS-generated.
Tests cover successful PIN setup/verify, wrong PIN/proof/signature/identity/tag,
state failures, bounded/truncated framing, timeout, credential persistence and
permissions, endpoint changes and revocation before socket contact, and
noninteractive CLI rejection. Existing audio/config tests remain intact.

Mocks are **not proof of Apple TV or HomePod compatibility**. No ARM64/hardware
validation, manufacturer attestation, external security audit or real encrypted
audio session is claimed. Actions remains manual-only (`workflow_dispatch`).
