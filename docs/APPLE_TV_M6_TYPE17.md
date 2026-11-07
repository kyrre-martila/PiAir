# Apple TV pair-setup M6 type 17 investigation

**Result: diagnostics only, not a compatibility fix.** Reviewed 2026-11-07
from main `fafe3d4` (merged PR #6). Type 17 (`0x11`) remains rejected.
No receiver was contacted, pairing retried, remote record changed, or playback
started during this investigation. No successful Apple TV pairing is claimed.

## Hardware report and current boundary

The reported receiver is Apple TV 4K, AppleTV6,2, tvOS 26.6.1. PIN display and
SRP progressed to an AEAD-decrypted M6 containing unexpected type 17. The report
establishes its presence, not its contents, length, format, or optionality.
The field allowlist failed **before** the required identity/signature checks;
the report therefore does not establish a valid receiver signature. M5 may
already have enrolled the controller remotely; no verified local credentials
were saved. Stop attempts and inspect receiver-side records before any explicit
retry, as described in [PAIRING.md](PAIRING.md#collecting-safe-hardware-pairing-diagnostics).

## Protocol evidence

These sources describe different evidence levels. No third-party implementation
source is copied or incorporated into PiAir.

| Source | What it establishes | What it does not establish |
| --- | --- | --- |
| [Apple HAP Non-Commercial R2 (2019-07-26), public mirror](https://forum.iobroker.net/assets/uploads/files/1634848447889-apple-spezifikation-homekit.pdf), §5.6.6, §5.15 table 5-6, §14.1.1 | M6 decrypts into identifier, Ed25519 public key and signature; the signature binds AccessoryX, identifier and public key. TLV types and method values are distinct namespaces. Generic HAP TLV rules say to ignore unrecognized types. | Table 5-6 does not assign `0x11` or define its format/length. The generic ignore rule does not explain Apple TV AirPlay-specific metadata or authentication policy. PiAir deliberately keeps its stricter stage allowlists for this investigation. |
| [Apple HomeKitADK, HAPPairing.h](https://github.com/apple/HomeKitADK/blob/fb201f98f5fdc7fef6a455054f08b59cca5d1ec8/HAP/HAPPairing.h), public Apple implementation referring to HAP R14 | Separates method enumeration, pairing TLV enumeration and flags. The published pairing TLV enumeration includes session ID `0x0e` and flags `0x13`, but no `0x11`. | It is not a tvOS 26 AirPlay specification and does not define this extension. Absence is not proof that Apple TV cannot use it. |
| [pyatv 0.18.0, hap_tlv8.py](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/auth/hap_tlv8.py) and [hap_srp.py, step3/step4](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/auth/hap_srp.py) | Names `0x11` **Name** under an explicitly uncertain Apple-internal comment. Optional M5 name data is an OPACK dictionary. Its client reads the three identity fields from M6 and ignores other fields. | M5 usage is not an M6 schema. `step4` leaves M6 signature verification as a TODO, so successful use of that client alone cannot establish PiAir's full verification requirements. |
| [pyatv AirPlay server_auth.py, _m5_setup](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/server_auth.py) | A **commented-out** M6 example suggests type 17 can contain an OPACK dictionary with name, model, account and address-related data. This suggests richer peer metadata, not necessarily a plain UTF-8 name. | The example is not emitted by the mock. It supplies no normative required keys, lengths, optionality, firmware provenance, or proof of an interoperable M6 extension. Do not collect those privacy-sensitive values. |
| [basmilius/apple-protocols, pairing.ts, AccessoryPair.m6](https://github.com/basmilius/apple-protocols/blob/d146f9835201f044b403d639ae989634fee10381/packages/common/src/pairing.ts) | A separately implemented client decrypts M6 and verifies the HAP Ed25519 identity transcript while selecting the three required fields from the decoded map. This corroborates preserving the signature independently of extra fields. | It does not parse or validate an M6 type-17 schema. Its tolerant map handling is not a definition of the field or evidence of compatibility with this reported firmware. |
| [Traego/go-atv, pairsetup.go, Finish](https://github.com/Traego/go-atv/blob/bae1283b43252e748cdef2699f53bfc41305f6f4/hap/pairsetup.go) | Describes name under TLVName among sealed M6 extras. | Explicitly follows pyatv and omits M6 signature verification; it is not independent corroboration of safe full verification or an extension schema. |
| [itsytv-core, PairSetup.swift](https://github.com/nickustinov/itsytv-core/blob/5538245fc3baa030c7da90d919893b4bee64ca05/Sources/ItsytvCore/Crypto/PairSetup.swift) | Another client extracts identifier/key after decryption. | This path is explicitly **Companion** pairing, does not establish AirPlay M6 type-17 semantics, and does not verify the M6 signature. |

## Assessment and decision

The strongest working hypothesis is Apple-specific peer metadata encoded as
OPACK. The reviewed public HAP definitions do not document type 17 as an
authentication-method extension. No reviewed evidence establishes that this
reported M6 uses a different authentication method or protocol variant; neither
can those possibilities be ruled out merely by the tag number. It is not
justified to equate type 17 with method 17, permissions, flags, or a plain name.

M6 AEAD authenticates bytes under the SRP-derived session key. That does not
establish the extra field's meaning, optionality, or relationship to policy.
The HAP Ed25519 transcript covers the derived signing material, identifier and
public key, **not arbitrary extra metadata**. A valid tag alone must not replace
signature verification. Names, accounts or addresses in possible metadata must
never replace the signed pairing identifier/key or the explicitly approved
receiver identity. Even signed peer claims would not grant authorization.

Given the missing M6 schema and firmware-specific evidence, this change does not
accept, ignore, decode, persist or use type 17. It adds no guessed OPACK or UTF-8
format rule, length limit, or new authentication method. Existing bounded TLV
framing, duplicate/fragment checks, outer allowlists, required fields, Ed25519
verification, exact stored pair-verify identity, receiver selection, approval
checks and fail-closed persistence are unchanged.

## Safe diagnostic and regression coverage

Only for a structurally valid decrypted **pair-setup M6**, rejection now includes
the presence and reassembled byte length of type 17, followed by the existing
sorted unexpected-type numbers. The length is measured across valid consecutive
255-byte TLV chunks. It is diagnostic data, not a trusted identity or format
validation. Invalid AEAD or malformed TLV framing yields no type-17 length.
Outer M6 and pair-verify failures retain their existing type-only diagnostics.
No values, names, PINs, credentials, keys, signatures or raw bodies are logged.

Tests use synthetic loopback receivers only: baseline signed M6 succeeds;
otherwise valid M6 with opaque type 17 fails (including a fragmented value);
truncation, empty/duplicate fields and oversized messages fail structurally;
unrelated unknown types and outer type 17 fail; each missing required identity
field and invalid signature fails. A type-17 failure cannot reach credential
persistence, leaves selection intact and closes the socket without another
request. Pair-verify also continues to reject type 17. There is no test claiming
an accepted documented extension: no sufficiently defined extension was found.

## Evidence still needed for acceptance

A future change needs a reliable AirPlay M6 definition or independently
corroborated receiver behavior identifying the type-17 schema, format/length
rules (if any), optionality and security/policy role for the applicable pairing
profile. An interoperable client that verifies the M6 signature and deliberately
handles this field would strengthen that evidence; generic unknown-field
tolerance alone does not establish semantics. Establish whether the metadata is
outside the signature transcript or uses an extended transcript on that profile.

Safe field presence/length can help scope that investigation but cannot prove
semantics. Do not export raw decrypted payloads to obtain evidence. Any future
hardware validation requires the possible partial enrollment to be resolved
first, explicit authorization for an attempt, and the existing physical receiver
verification and allowlist. Mock tests do not establish hardware compatibility.

## Validation performed

On x86_64 Linux with Rust 1.99.1: `cargo fmt --check`,
`cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`
(84 unit tests and 11 integration tests), and `git diff --check` pass.
The initial sandboxed test run had 33 failures at loopback socket creation
(`Operation not permitted`); rerunning outside that socket restriction passed
the complete suite. An initial test compilation error from a Debug-requiring
assertion was corrected using a boolean assertion, preserving the existing
non-Debug selection type. No hardware or ARM64 validation was performed.
