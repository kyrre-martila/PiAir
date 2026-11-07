# Apple TV pair-setup M6 type 17 investigation

**Result: narrowly scoped, evidence-informed compatibility change requiring
real Apple TV hardware validation.** Reviewed 2026-11-07 from main `fafe3d4`
(merged PR #6). Type 17 (`0x11`) is optional opaque, untrusted metadata only
inside successfully AEAD-decrypted pair-setup M6. This is not a verified protocol
schema or a proven hardware fix.
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
| [Apple HAP Non-Commercial R2 (2019-07-26), public mirror](https://forum.iobroker.net/assets/uploads/files/1634848447889-apple-spezifikation-homekit.pdf), §5.6.6, §5.15 table 5-6, §14.1.1 | M6 decrypts into identifier, Ed25519 public key and signature; the signature binds AccessoryX, identifier and public key. TLV types and method values are distinct namespaces. Generic HAP TLV rules say to ignore unrecognized types. | Table 5-6 does not assign `0x11` or define its format/length. The generic ignore rule does not explain Apple TV AirPlay-specific metadata or authentication policy. PiAir accepts only type 17 as an additional opaque field in decrypted M6; all other stage allowlists stay strict. |
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

This change follows the generic HAP extension-tolerance rule and observed
client handling only for the reported decrypted M6 type 17. PiAir treats it as
optional opaque, untrusted metadata. It does not parse its internal format,
interpret, persist, log, or use its contents for authentication, identity or
authorization. The TLV parser still validates framing and reassembles chunks;
there is no OPACK parser, guessed UTF-8 rule, or invented field-specific length
rule. Existing nonempty-value policy and the 8 KiB message bound remain intact.
This does not establish the field's protocol-defined optionality or schema.

Acceptance applies only after M6 AEAD succeeds. All three required identity
fields and the full HAP Ed25519 signature must still verify before any credential
is returned. Only the signed identifier/key bind enrollment to the approved
receiver. Other unexpected types, outer M6 type 17, earlier setup stages and
pair-verify type 17 remain rejected. Approval/selection rechecks and fail-closed
persistence are unchanged. No retries, remote cleanup or playback are added.

## Regression coverage

Synthetic loopback receivers exercise baseline M6 and valid signed M6 with
opaque type 17 of lengths 1, 255, 256 and 510 bytes, including consecutive TLV
fragments. The fixture bytes are intentionally opaque, not a claimed schema.
Tests verify the returned and persisted identifier/key match the signed identity,
and that setup itself does not write credentials before explicit persistence.

With type 17 present, invalid signatures, invalid AEAD, each missing required
field and unrelated unknown types still fail and save no credentials. Malformed
TLV headers/lengths, empty/duplicate fields and oversized messages fail. Type 17
in outer M6, setup M2 or pair-verify M2 fails. Failures after M5 preserve the
partial-enrollment warning and close without another request. No metadata
contents or lengths are logged by the compatibility handling.

## Evidence and hardware validation still needed

The sources above support bounded extension tolerance while preserving the HAP
identity transcript; they do not define a verified Apple TV M6 type-17 schema.
A reliable AirPlay definition or independently corroborated receiver behavior
would still be needed to interpret this field, assign format/length rules, or
claim a firmware-specific protocol meaning. Generic field tolerance and mock
success do not establish real hardware compatibility.

Do not export raw decrypted payloads to obtain evidence. Any future hardware
validation requires possible partial enrollment to be resolved first, explicit
authorization for an attempt, and existing physical receiver verification and
allowlisting. No hardware test was performed for this change.

## Validation performed

On x86_64 Linux with Rust 1.99.1: `cargo fmt --check`,
`cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`
(84 unit tests and 11 integration tests), and `git diff --check` pass.
No hardware or ARM64 validation was performed.
