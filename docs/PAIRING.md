# Receiver pairing, identity and compatibility

PR #3 implements independently written persistent HAP pairing and pair-verify,
not AirPlay audio transport. No sender implementation source from OwnTone,
Autostream, airplay2-rs or other projects is copied, translated or incorporated.
Protocol documentation, public wire observations and numerical test vectors are
used as evidence. **No real receiver has been contacted or validated by PiAir.**

This document records the PR #3 baseline. PR #4 supersedes its statements that
verify always drops the socket and that encrypted application I/O is absent:
[the current session document](AIRPLAY_SESSION.md) describes live verified socket
handoff, encrypted OPTIONS, revocation checks and the still-disabled media path.
Default `authenticate` still closes after verification. Prompt 4c also adds an
explicit realtime stream-test and refines bit-51 policy: with explicit persistent
HAP bit 46 it is no longer blanket-blocked, based on the pinned sender analysis.
The older matrix/decision discussion below is historical; current supported and
rejected profiles are in AIRPLAY_SESSION.md. Signed identity and approval checks
remain unchanged.

## Evidence ledger

Research reviewed 2026-11-07. These evidence levels must not be conflated.

### Public specifications: verified protocol definitions

- **[HAP R2, 2019-07-26, public mirror](https://forum.iobroker.net/assets/uploads/files/1634848447889-apple-spezifikation-homekit.pdf)**:
  §5.5 defines SRP-6a 3072/SHA-512 and numerical vectors; §5.6 defines persistent
  setup including a displayed or existing setup code, and transient completion at
  M4; §5.7 defines pair-verify; §5.9 defines AEAD; §5.15 tables 5-3/5-6/5-7 define
  methods, TLVs, and transient/split flags. §5.6.6 authenticates the identifier
  delivered inside M6, then saves it with the receiver public key. It does **not**
  specify equality with AirPlay mDNS `pi`. HAP specifies HTTP endpoints; it is not
  a specification of every Apple TV/HomePod AirPlay firmware behavior.
- [RFC 5054](https://www.rfc-editor.org/rfc/rfc5054),
  [RFC 5869](https://www.rfc-editor.org/rfc/rfc5869),
  [RFC 8439](https://www.rfc-editor.org/rfc/rfc8439),
  [RFC 7748](https://www.rfc-editor.org/rfc/rfc7748), and
  [RFC 8032](https://www.rfc-editor.org/rfc/rfc8032) specify the standard group,
  HKDF, AEAD and curve/signature primitives. Rust crates supply those primitives.
- [Apple's HomePod access documentation](https://support.apple.com/en-gb/guide/homepod/apdb68d3dec5/homepod)
  documents network/Home membership restrictions and optional speaker passwords.
  An AirPlay password, Home membership and a HAP setup code are different concepts;
  this guide does not specify their on-wire implementation.

### Independently documented behavior and implementation reports

- [AirPlay discovery fields](https://openairplay.github.io/airplay-spec/service_discovery.html),
  [feature assignments](https://openairplay.github.io/airplay-spec/features.html),
  and [status flags](https://openairplay.github.io/airplay-spec/status_flags.html)
  describe bit 38 (CoreUtils), 43 (system), 46 (HK/access control), 48 (transient)
  and 51 (unified pairing/MFi), plus PIN/password/access-control metadata. These
  are unofficial mappings, not cryptographically authenticated negotiation.
- [HomeKit-based AirPlay pairing](https://openairplay.github.io/airplay-spec/pairing/hkp.html)
  documents persistent versus transient key sources and MFi material. Bit 51
  advertises a capability: it does not establish that every operation must use
  MFi, nor that method 0 is always safe/accepted. PiAir conservatively blocks that
  unresolved policy rather than silently bypassing attestation.
- [Public AirPlay pairing wire transcript](https://github.com/openairplay/ap2-sender/blob/master/pairing.txt)
  records PIN-start, setup and verify with HKP 3 and HTTP responses. Only protocol
  observations were consulted; no implementation or transcript payloads are
  incorporated. The capture is evidence for that exchange, not all firmware.
- [pyatv protocol documentation, AirPlay 2 authentication](https://pyatv.dev/documentation/protocols/#airplay-2)
  describes multiple HAP-based paths and labels parts of its AirPlay description
  incomplete. Its account is not a device/version conformance specification.
- [cidermill-airplay 0.1.5 pairing documentation](https://docs.rs/cidermill-airplay/0.1.5/cidermill_airplay/pairing/index.html)
  reports transient rejection by tested Apple TVs, a HomePod mini success with an
  iPhone-shaped request, and hardware pair-verify using a Companion-derived
  credential. These are the author's reports, not PiAir hardware validation.
  [Its documented wire-profile comparison](https://docs.rs/cidermill-airplay/0.1.5/cidermill_airplay/pairing/enum.TransientStyle.html)
  describes differing HKP values, content types, flags and preliminary abandoned
  verifies. No source implementation was inspected or copied.
- [UxPlay's receiver-side crypto account](https://github.com/FDH2/UxPlay/wiki/crypto)
  documents a different legacy path, including AES-CTR. A similarly named
  `/pair-setup` endpoint does not make legacy and HAP interchangeable.

### Unresolved assumptions and blockers

There is no basis for selecting an authentication method solely from “Apple TV”,
“HomePod”, “HomePod mini”, model/version text, name or IP. Modern Apple TVs are
candidates for persistent HAP/verify, but firmware and receiver policy may require
other enrollment. Companion-derived credentials in a report do not establish
AirPlay PIN enrollment. Companion enrollment/import is not implemented here.

HomePod/mini commonly motivate transient pairing, but there is no sufficiently
validated single AirPlay transient wire profile in PiAir. HAP defines its SRP
semantics; it does not define AirPlay's HKP/header/probe variants. This is the
reason transient remains unsupported, **not** a claim that SRP is unimplementable
or that transient sessions inherently require persistent identity. PiAir does not
send speculative abandoned verifies, try fixed codes, or cycle variants.
The comparison specifically distinguishes classic HKP 1 from an observed HKP 4
profile preceded by abandoned HKP 6/3 verifies. These are wire-mode choices,
not HAP specification revision numbers.

The setup-code profile below is a valid HAP enrollment mechanism, not a claim
that HomePod supplies a usable printed code or permits that enrollment. Home
access restrictions, MFi policy, and password-to-protocol mapping remain blockers.
No protocol-model mapping or hardware compatibility is claimed from mock success.
MFi attestation and FairPlay sender/media setup are distinct from HAP enrollment;
passing HAP alone does not prove that a future encrypted audio setup can proceed.
Neither MFi nor FairPlay is implemented.

## Compatibility matrix

All categories below require real hardware validation before compatibility claims.

| Receiver / protocol category | Requested method | Implementation | Authentication guarantee | Persistent credentials | Hardware validation |
| --- | --- | --- | --- | --- | --- |
| HK bit 46, no bit 51, usable displayed PIN; Apple TV candidate | `pin-display` | Implemented: PIN-start + persistent method 0 M1–M6 | Physical PIN-authenticated signed ID/key enrollment | Yes, after verified M6 | Required; no tvOS/model verified |
| Same HK category with an independently obtained existing HAP code | `setup-code` | Implemented: method 0 M1–M6 without PIN-start | Code-authenticated signed ID/key enrollment | Yes, after verified M6 | Required; usable HomePod code not established |
| Previously enrolled receiver passing the current capability policy | Same selected persistent profile; `authenticate` | Implemented: pair-verify M1–M4 | Stored receiver ID/key, fresh X25519 and mutual signatures | Reads existing; never replaces on verify | Required; no live session retained |
| System/transient bits 43/48; HomePod/mini candidate | `transient` | Recognized, rejected before connection | None in PiAir; generic SRP would prove a code, not durable identity | Never from transient | Required; wire-profile blocker above |
| Bit 51, with or without bit 46 | Unified/MFi or policy-specific | Unsupported; no inference that MFi is universally mandatory | No MFi chain/attestation validation | No new enrollment | Required |
| CoreUtils 38 only, legacy bit 27, unknown combinations, password/Home access restrictions | Unresolved / legacy / FairPlay | Unsupported; no fallback | None | No new enrollment | Required |

Bit 40 (buffered audio) is reported only as an advertised playback-related feature;
it is not a pairing decision, verified compatibility, or successful audio session.
HKP 3 is an AirPlay header choice, HAP R2 is the referenced specification revision,
and advertised `protovers` is another field. They are not interchangeable versions.

## Explicit method decision and wire behavior

`compatibility::Capabilities` parses feature aliases numerically, bounded status
flags, exact password booleans, decimal ACL and protocol version. Malformed fields
cannot disappear into permissive defaults. Contradictory aliases or password
signals fail closed. Unknown non-pairing feature bits remain uninterpreted data.
Missing/unsupported pairing evidence never becomes a default successful method.

`Capabilities::decide(requested)` is a pure, tested local policy. It never searches
for another method. Both persistent profiles require bit 46 without bit 51.
Bit 38/43/48 alone is insufficient evidence for persistent enrollment. Password
requirements, nonzero ACL and unknown protocol versions are refused; missing
optional metadata is retained as unknown, not proof of unrestricted access.
`protovers` 1.0/1.1 or absent is permitted for this candidate profile only.
These conservative restrictions are PiAir policy, not invented protocol mandates.

Selection persists the requested method, full parsed capability snapshot,
optional advertised `pi`, stable ID, exact address set and port. The live boundary
rechecks that policy and snapshot immediately before every connection. A selected
method cannot change silently. `receivers` displays decisions/rejection reasons
without probing. `select --method transient` is an explicit unsupported-method
error, not an automatic fallback option. There is no `auto` method.

Authentication POSTs use HTTP/1.1, numeric pinned Host, persistent connection,
CSeq, HKP 3 and octet-stream TLV8. Responses accept the existing bounded HTTP/RTSP
200 framing, with matching CSeq when supplied (mandatory for RTSP). No HTTP/RTSP
request retry or header-version negotiation occurs. This corrects the earlier
RTSP-only request assumption; firmware acceptance still needs controlled tests.
HAP's URL table does not define `/pair-pin-start`; the AirPlay capture does not
make that request mandatory for every HAP accessory. Encrypted application
control I/O is not implemented.

`pin-display` requests `/pair-pin-start`, then asks for 4..8 digits after M2.
`setup-code` goes directly to `/pair-setup`, then asks for an existing
`XXX-XX-XXX` code, preserving the dashes in SRP. It never tries PIN-start after a
failure. Both send method 0 and require full M6. Optional zero flags in M2 are
accepted; transient/split/unknown flags, extra MFi material, bad states/proofs,
and unexpected nested fields are rejected. No M4-only exchange returns credentials.

## Identity and approval boundary

The authorization identity is still the exact canonical
`device:<12 lowercase hex digits>`. Display names, hosts, IPs and unverified
service IDs cannot replace it. Approval is explicit `receiver_allowlist` editing;
selection does not grant approval. All network operations require interactive
exact-ID confirmation and currently approved selection. No status/config/audio
command opens receiver connections. Discovery is passive DNS-SD, with no probing.
**Samsung The Frame and every other unapproved receiver are never contacted.**

The private `Wire` owns the sole production TCP connect, taking literal pinned
addresses only (no hostname/DNS path). It reloads current approval and persisted
selection, rejects changed/ambiguous IDs and endpoints, and makes one bounded
connection attempt. No redirects, retries, alternate addresses or unauthenticated
fallback. IPv6 link-local scope handling is unsupported. Shared-address conflicting
advertisements cause rejection. Discovery comparison permits only the operational
status changes defined below; persisted approval comparisons remain exact.

Advertised `pi` is optional, untrusted discovery metadata, pinned for change
detection. It is **not** used as an authenticated identifier or mandatory expected
M6 value. After verifying SRP, M6 AEAD and the receiver signature, PiAir enrolls the
actual case-sensitive signed identifier/key under the approved stable device ID.
Its bounded identifier policy currently accepts ASCII letters/digits/colon/dash;
other HAP UTF-8 identifier forms remain unsupported. Pair-verify requires that
exact enrolled identifier and public key, independent of advertised `pi`. A
changed signed identifier or key is an error, never credential replacement.

The initial physical PIN/code proves knowledge at the approved endpoint; it does
not cryptographically prove an mDNS MAC address or manufacturer identity. If mDNS
is spoofed during initial enrollment, user approval and the independently obtained
physical code are the trust anchor. After enrollment, stored key verification
protects against endpoint impersonation. A changed advertised `pi` requires
investigation and explicit local unpair/re-enrollment; endpoint reselection alone
cannot change the enrolled identity. Do not paste a code from an unverified host.

## Sequences, timeouts and secret lifetime

```mermaid
sequenceDiagram
    actor User
    participant CLI
    participant Boundary as Approval boundary
    participant R as Selected receiver
    User->>CLI: approve stable ID; select explicit persistent method
    User->>CLI: pair ID; type exact ID
    CLI->>Boundary: reload approval, selection and passive snapshot
    Boundary->>R: one bounded TCP connection
    opt selected pin-display only
        CLI->>R: POST /pair-pin-start
    end
    CLI->>R: /pair-setup M1 method 0
    R-->>CLI: M2 salt, B, optional zero flags
    User->>CLI: hidden physical PIN or existing setup code
    CLI->>R: M3 A + proof
    R-->>CLI: M4 receiver proof
    Note over CLI: verify proof; never treat M4 as persistent success
    CLI->>R: M5 AEAD(controller ID, key, signature)
    R-->>CLI: M6 AEAD(actual receiver ID, key, signature)
    Note over CLI: verify AEAD + signature; enroll actual signed ID, not pi
    CLI->>CLI: recheck approval/selection; save version 2 credentials
    Note over CLI,R: close socket; no audio session
```

```mermaid
sequenceDiagram
    actor User
    participant CLI
    participant R as Currently approved receiver
    User->>CLI: authenticate ID; type exact ID
    Note over CLI: current boundary, capability and discovery checks
    CLI->>R: pair-verify M1 fresh X25519 key
    R-->>CLI: M2 ephemeral key + AEAD(signed receiver ID)
    Note over CLI: require stored ID/key; reject zero DH, signature/tag failures
    CLI->>R: M3 AEAD(controller ID/signature)
    R-->>CLI: M4 success
    Note over CLI: derive directional control keys; close socket and drop keys
```

`discovered`, `approved`, `selected`, and `paired` are separate facts. `paired`
means valid stored enrollment, not current authentication/liveness. Read-only
commands never claim authenticated/connected. Only completed pair-verify reports
an authenticated exchange, then closes immediately. Playback remains unimplemented.

Connect and each I/O are limited to 3 seconds; automated phases have 15-second
absolute deadlines, recalculated for partial I/O. User PIN think time is excluded.
Messages/headers are bounded at 8 KiB. Owning handshake APIs close the socket on
success/error. Failed wire/control framing is poisoned; no retry/reuse after
failure. Owned secret storage is zeroized; the PIN drops immediately after SRP
calculation. No payload/private-key/PIN logs or secret Debug implementations.
Complete process-memory erasure is not guaranteed: serde/bigint/allocator copies
can remain, and num-bigint SRP arithmetic is not constant-time. Proof comparisons,
AEAD and signature verification use established library verification. No security
audit or protection against hostile same-user local processes is claimed.

## Credentials, migration and partial enrollment

`receiver-state/` is separate from ordinary configuration, next to its resolved
path. Linux requires owned nonsymlink 0700 directory and regular single-link
0600 files. Reads use O_NOFOLLOW, strict parsing and a 16 KiB bound. Same-directory
temporary writes are synced; credentials use no-clobber persistence. Selection
uses atomic replacement. Parent config paths and same-user processes are trusted.
The parent directory must exist. Multiple config files in one directory share state.

Version 2 credentials distinguish the actual authenticated `pairing_id` from
optional untrusted `advertised_pi`, and bind both to the approved stable ID.
Old PR #3 version 1 credentials are rejected with explicit recovery instructions;
there is no silent reinterpretation or reset. Old selection files require
reselection. Use `unpair <id>` to remove old/corrupt local credentials, inspect
receiver-side records, then select and enroll explicitly again. This pre-merge
schema change does not silently migrate a trust decision.

**Filesystem permissions are the primary local protection; no encryption at rest.**
Do not commit or expose backups of receiver-state. Corruption/permissive state
fails closed. Inspect and repair ownership/permissions manually where appropriate.
Unpair never contacts the receiver, clears selection, or revokes the allowlist;
also remove the allowlist entry to forbid future connections.

Once M5 is attempted, the receiver may enroll the controller even if M6 is lost,
invalid or local persistence fails. Errors explain that possible partial enrollment.
Nothing is saved before successful M6 verification. Approval/selection changes
before persistence prevent saving; no automatic enrollment retry or remote cleanup
occurs. Inspect local state and remove stale controller records using receiver
controls before another attempt. File sync failure can leave an installed file;
check state rather than assuming rollback. Revocation is checked before connections
and enrollment persistence, without an instant background revocation watcher.

## Collecting safe hardware pairing diagnostics

A reported Apple TV 4K (AppleTV6,2, tvOS 26.6.1) displayed a PIN but
failed the decrypted M6 field allowlist with type 17 (`0x11`).
[The type-17 evidence ledger](APPLE_TV_M6_TYPE17.md) records the research,
metadata hypothesis and missing schema evidence. Only successfully decrypted
pair-setup M6 now tolerates optional opaque type 17, with all required identity
and signature checks preserved. This is an evidence-informed compatibility
change requiring real Apple TV validation, not a verified schema or proven fix.

1. If an earlier attempt reached M5/M6 or reported possible partial enrollment,
   stop PIN attempts first. `selected` and `approved` with `paired=false` means
   no valid local credential; it does not prove the receiver has no controller record.
   Inspect receiver-side controller/pairing records using supported receiver or
   administrator tools. Record only whether a stale record exists, without
   exporting identifiers, keys, or credentials. tvOS may not expose the relevant
   AirPlay records in its UI; if unavailable, consult receiver support rather
   than guessing, repeatedly pairing, or resetting unrelated records.
2. Only after resolving possible partial enrollment and explicitly deciding to
   retry, run the updated binary's existing `pair` command once on the physically
   verified, selected, allowlisted receiver, from an interactive terminal. Enter
   the PIN only at the hidden prompt. No debug/trace logging or packet capture is
   needed. Do not automate retries or run playback as a diagnostic.
3. Copy only the command failure's stage/context chain, unexpected TLV type
   numbers, plus PiAir commit, receiver model and firmware. Type-17 metadata is
   ignored only inside AEAD-decrypted M6 and its contents and lengths are not
   logged. For example:
   `pair-setup M6 decrypted payload: unexpected pairing TLV types: [19]`
   is an illustrative diagnostic, not the observed field or permission to accept it.
   Outer TLV failures, decryption, SRP proof and signature verification have
   separate context. Review output before sharing and redact local paths and
   device identifiers. Never share PINs, raw packets/TLV values, SRP material,
   keys, signatures, encrypted bodies, or files from `receiver-state/`.
4. After a partial-enrollment warning, stop again and check receiver-side records
   before any further explicit attempt. PiAir does not retry, unpair remotely,
   remove records, or start playback on this failure. Local `unpair` cannot clean
   receiver records. Strict field validation and authenticated identity verification
   still apply; unexpected type numbers alone do not establish their semantics.

## Controlled commands and the PR #4 boundary

Only use a physically verified, intentionally allowlisted receiver. IDs are examples:

```sh
piair --config config.toml receivers
# Independently verify the physical ID and explicitly edit receiver_allowlist.
piair --config config.toml select device:aabbccddeeff --method pin-display
piair --config config.toml pair device:aabbccddeeff
piair --config config.toml authenticate device:aabbccddeeff
# Alternative ONLY when a real HAP setup code has independently been obtained:
piair --config config.toml select device:aabbccddeeff --method setup-code
# Removal is local; remote cleanup and allowlist revocation are separate:
piair --config config.toml unpair device:aabbccddeeff
```

PR #4 may build on the exact-ID boundary, explicit capability decision, persistent
HAP setup/verify compositions, schema-2 identity binding, bounded TLV/HTTP parsing,
control-key derivation/framing and local tests. Current APIs intentionally close
connections; they do **not** hand PR #4 an authenticated live audio channel.
Retaining one will require a session owner binding its socket and keys to that
same approved identity, live revocation policy, and controlled hardware evidence.

PR #4 must not assume Apple TV/HomePod compatibility, automatic reconnection,
transient credentials, Home/Companion authorization, MFi/FairPlay, encrypted control
I/O, timing, media-key negotiation or playback. Those are unresolved or absent;
streaming must not hide them behind an authentication fallback. Report model and
firmware with sanitized errors only; never publish authentication bodies or keys.

## Validation

`cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, and
`cargo test --locked` run locally. Tests use loopback mocks and synthetic snapshots,
never real receivers. Both persistent code sources, actual-ID versus pi handling,
wrong codes/proofs/tags/signatures, state/downgrade errors, malformed capabilities,
unsupported transient/MFi paths, current authorization, cleanup, storage/migration
and read-only dispatch are covered. Apple's HAP R2 §5.5.2 numerical vector checks
A/K independently of the mock; fixture data is not implementation source.
GitHub Actions stays manual-only (`workflow_dispatch`). Mock success is not
Apple TV/HomePod interoperability evidence or ARM64/hardware validation.


## Selection validation across operational status changes (PR #9)

`Selection::from_receiver` still requires one unambiguous AirPlay service,
matching canonical device identity, a valid optional `pi`, literal nonempty
addresses and a nonzero port. It parses both feature aliases and both status
aliases numerically, rejects contradictions/malformed fields, and runs
`Capabilities::decide` for the explicitly selected method on **every connection**.
Persistent HAP still requires feature bit 46; password protection, unresolved
ACLs, unknown protocol versions and transient profiles are rejected. Feature
words remain exactly pinned, including unknown bits: additions and removals both
require review. Audio/ALAC feature downgrades therefore fail before connecting;
stream-test also requires bits 9/19 and retains authenticated GET /info profile
validation. This change adds no streaming profile or authentication fallback.

The previous whole-Selection comparison treated changing status as a new
receiver. Discovery now uses a dedicated comparison; `PartialEq` is unchanged
for the persisted selection recheck before connecting, live revocation checks,
media leases and enrollment handoff. ID, optional advertised `pi` (including
presence), address set, port and selected method must match exactly. Password,
ACL and protocol fields also retain exact values/presence. Only the following
`flags`/`sf` bits may change in either direction when both snapshots contain flags:

| Bit | Operational interpretation |
| --- | --- |
| 2 | Audio cable attached |
| 11 | Relay/current-track availability |
| 12 | Silent primary |
| 13 | Synchronization group leader |
| 14 | Synchronization buddy unreachable |
| 17 | Receiver session active |
| 20 | Observed Apple TV local TV playback status; provisional interpretation |

The [unofficial status reference](https://openairplay.github.io/airplay-spec/status_flags.html)
defines bits 2 and 11–17. The
[receiver implementation](https://github.com/openairplay/airplay2-receiver/blob/6c343d3679ddb561c61566985acaaf587d0a3bd3/ap2/bitflags.py)
corroborates those names; its session lifecycle toggles bit 17. These are
reverse-engineered references, not an Apple specification. Bit 20 is absent
from their definitions: the supplied Stua observation is the evidence for this
specific exception, **not** proof of a general bit-20 meaning. The reported
receiver ID, IPv4 address, port, `pi` and model stayed unchanged
while flags changed `0x18644` ↔ `0x118644`. Model/name are informational discovery
metadata, never authorization or authenticated identity.

Bits 3/9 (PIN/pairing), 7 (password), 10 (HomeKit access control), cloud/support
and subscription/library bits, problem/configuration bits and every other
unknown bit remain pinned. Removing or introducing the flags field also fails.
The exceptions report topology/activity rather than granting access; they never
relax allowlisting, endpoint pinning, explicit confirmation, credential binding,
HAP signature verification or encrypted control. No status bit authorizes a
receiver or triggers selection, pairing or endpoint changes. Operational
acceptance does not establish that a receiver is available or that playback will
succeed, nor grant permission to interrupt existing playback.

The serialized selection format and raw status snapshot are unchanged. Existing
idle or playing selections load directly; no migration, rewrite, automatic
reselection or credential replacement occurs. Tests verify the saved bytes and
credential key remain unchanged. Regression tests use loopback only: both
observed transitions, additional operational bits, exhaustive single-bit policy,
changed/missing `pi`, endpoint changes, conflicting identities/services,
capability downgrade, contradictory advertisements, changed persisted method,
and allowlist revocation. Existing credential/signature and live revocation tests
remain applicable.

No hardware was contacted, no playback was started, and no OwnTone, Autostream
or system services were changed. Remaining assumptions are the provisional
bit-20 interpretation and the reverse-engineered meanings above; genuine changes
to other fields still fail closed and require explicit review. Realtime
negotiation and Apple TV playback remain unverified by these local tests.

Validation from fresh main `1e60b6b` (merged PR #8), Rust 1.99.1:

- `cargo fmt --check` — passed.
- `cargo clippy --all-targets --locked -- -D warnings` — passed.
- `cargo test --locked` — passed: 91 unit tests and 11 integration tests.
- `git diff --check` — passed.

Rust tooling and official Debian ALSA development headers were installed into
workspace-local directories. The final test run required loopback socket access;
an initial sandboxed invocation failed with `PermissionDenied` on mock binds,
then the same suite passed with local sockets enabled. No hardware access was
needed or attempted.
