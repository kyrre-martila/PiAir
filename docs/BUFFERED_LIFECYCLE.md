# Single-receiver buffered lifecycle: fail-closed foundation

> PR #43 update: see [current compatibility evidence, staged controls and remaining blockers](AIRPLAY2_COMPATIBILITY.md). The body below records the earlier foundation; its future-work statements are historical. Audio remains gated.

This change takes Prompt 42's explicit fallback: **live buffered playback remains
unavailable**. It implements an opt-in, authenticated initial PTP SETUP diagnostic
and its resource lifecycle. It does not implement or claim a complete playable
AirPlay 2 session. No physical receiver or hardware diagnostic was used in this
change. TCP delivery, a mock, RTSP 200 and peer timing activity cannot establish
verified audible playback.

## Architecture and states

`receiver/lifecycle.rs::Session` is the sole new buffered diagnostic owner. It
owns one authenticated `Wire`, one mandatory-authorized PR #41 `ptp::Service`,
one encrypted event worker, the session token, fresh random clock identity,
fresh UUID/URI and cancellation flag. It never owns or opens a media socket.
The existing `receiver/session.rs` realtime/NTP engine is preserved; its legacy
buffered entry point returns the same closed-gate reason.

```mermaid
stateDiagram-v2
    [*] --> Authorization
    Authorization --> Authentication: current explicit selection
    Authentication --> Negotiation: same-socket authentication
    Negotiation --> Timing: authenticated info and initial SETUP validated
    Timing --> Stopping: diagnostic ends or cancellation
    Negotiation --> Stopping: partial setup failure
    Authentication --> Stopping: authentication failure
    Authorization --> Stopping: failure or cancellation
    Stopping --> Closed: cleanup acknowledged or no setup attempted
    Stopping --> Failed: operation or cleanup failure
```

Prebuffering, Playing and Paused are explicit reserved states. There is **no
legal transition into them**, including from Timing. No timer, successful
SETPEERS, anchor response, estimator result or caller-supplied boolean can open
this gate. Start/observe/stop on a stale owner is rejected; Drop cleanup is
idempotent. A fresh owner gets a fresh clock identity and UUID; media keys and
counters from another owner cannot enter this diagnostic because it has none.

Current sequence:

1. Require an exact receiver ID, current allowlist membership, persisted selection,
   explicitly selected authentication method and advertised buffered/PTP features.
2. Require terminal confirmation for `--setup-check`; revalidate the selected
   endpoint through `Wire::connect`. Never select, pair, downgrade or fall back.
3. Run existing persistent pair-verify or explicitly selected transient
   authentication on that same control connection. Transient authentication has
   its existing lack of persistent receiver identity assurance; it is not pairing.
4. Encrypted GET `/info`: require matching `deviceID`, buffered/PTP feature bits,
   non-conflicting authenticated ALAC24/48 kHz/stereo format ID 21 and a supported
   access policy. Do not use realtime-format checks to select buffered audio.
5. Send a bounded initial SETUP with fresh session UUID, PTP clock identity and
   local control address. The offer intentionally omits unvalidated group,
   clock-port-override and device-MAC assumptions. Its acceptance is unverified.
6. Require binary-plist SETUP, a valid event port, matching optional PTP protocol,
   zero optional legacy timingPort, no streams and timingPeerInfo containing a
   nonzero/non-reserved/non-local signed-64-bit ClockID and **only the exact
   authorized receiver IP**. Multiple timing addresses are rejected, never tried.
   Missing ClockID is an explicit blocker, never derived from UDP or device IDs.
7. Connect the encrypted event channel only to that receiver; attach mandatory
   authorized PTP event/general sockets on standard UDP 319/320. Bind failure is
   fatal. Poll for at most one second, checking authorization, event health and
   cancellation every iteration. Each PTP poll is bounded by PR #41's limits.
8. Stop PTP and events; attempt authenticated same-session TEARDOWN under a 250 ms
   request budget; close control and zeroize its owned cryptographic state.

Parameterized Session headers are deliberately unsupported by this diagnostic;
plain tokens are bounded, validated and pinned. Changed tokens are rejected.
Request and response plists, key bytes and samples are not logged.

## What the protocol evidence establishes

References were inspected for protocol facts, not used as an implementation
skeleton. No GPL code was copied, translated, vendored or linked. This is not a
claim of personnel-separated clean-room development. All new runtime code uses
existing Apache-compatible dependencies; reference files are outside the source
tree and are not build inputs.

| Pinned evidence | Established observation | Limit |
| --- | --- | --- |
| [owntone-mini minimal e9b058448b7d921ed048790b45614685fd13802b, airplay.c](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/outputs/airplay.c#L4623) | Initial PTP offer uses timingPeerInfo/ClockID/Addresses and timingPeerList; ClockID is a signed eight-byte plist integer. Initial response requires eventPort and timingPeerInfo. | Reference response path selects an address; that alone does not supply or prove receiver clock identity/adoption. PiAir requires a ClockID and exact address, so compatibility may be narrower. |
| [Same revision, sequence table](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/outputs/airplay.c#L6603) | Initial SETUP → event/PTP establishment → RECORD → SETPEERS → command tables → stream SETUP → audioMode; accepted rate anchor gates buffered media. Feedback and teardown are separate control sequences. | Sequence observed in an implementation is not receiver conformance. Command-table/audio-mode requirements and safe pause/flush semantics remain unvalidated for this sender. PiAir stops before RECORD. Volume operations in the reference are intentionally excluded. |
| [Independent openairplay receiver 6c343d3679ddb561c61566985acaaf587d0a3bd3](https://github.com/openairplay/airplay2-receiver/blob/6c343d3679ddb561c61566985acaaf587d0a3bd3/ap2-receiver.py#L221) | Independently corroborates timingPeerInfo/Addresses and initial/stream SETUP separation. Its timingPeerInfo example has no ClockID. RECORD returns informational latency; SETPEERS consumes a bare address array with `/peer-list-changed`; anchor rate 0/1 and rtpTime are recognized. | No proof of Apple receiver servo lock or authenticated adoption. Empty 200 is acceptance only. Missing ClockID cannot be repaired by assuming the device ID equals the PTP identity. |
| [PR #41 timing ledger](PTP_TIMING.md) and [PR #40 transport ledger](BUFFERED_TRANSPORT.md) | IEEE layouts, local clock model, full-precision anchors, independent plist/AEAD/ALAC fixtures, type103 framing and bounded TCP primitives. | Component correctness does not validate a complete receiver playback profile. |

The audited main was `280f5ef697da63dc6a86245d5f1b89df508f3053`, merging PR #41.
The audit included session, buffered, ptp, wire, planning, profile, compatibility,
pairing and their tests. Historical reference conclusions remain historical;
this ledger describes the new diagnostic's exact scope.

## Readiness criterion and unresolved blocker

PTP `PeerObserved` is liveness only. A grandmaster sees requests, not the remote
servo's offset. The standalone follower estimator cannot prove that this receiver
has adopted PiAir's grandmaster. Delay traffic, elapsed time and control 200s
must never be promoted to readiness.

Before enabling a complete lifecycle, independent protocol evidence must define
how to establish and continuously validate the receiver's adopted clock identity
and usable sample-to-clock mapping on the authenticated session, including Apple
signaling/TLV semantics if required. Evidence must distinguish remote adoption
from local estimator stability and quantify freshness/discontinuity bounds.
There is currently no supported mechanism supplying this evidence; readiness is
therefore **always false**. No fake production or test-only clock-lock override
is provided to make a mock playback lifecycle pass.

Even after that blocker is resolved, the owner must validate two-stage stream
SETUP, command/audio-mode exchanges, codec/encryption/streamConnectionID/ports,
SETPEERS and successful fresh SETRATEANCHORTIME before the first media byte.
It must own the existing ALAC24/type103/ChaCha20-Poly1305 components through one
revocable media path, service PTP during control and each write, and implement
bounded prebuffer, first-frame marker, starvation handling, feedback, anchor
renewal and pause/resume/flush with explicit sample/nonce continuity. None of
those operations is claimed implemented or tested end-to-end by this change.
No media connection or generated test tone is exposed while readiness is closed.

## Cleanup and security boundaries

SETUP is marked attempted before the request is sent. Semantic validation or
resource attachment failure after RTSP 200 therefore attempts TEARDOWN. If
control framing/authentication fails, Wire invalidates and closes the channel;
if approval is revoked, even TEARDOWN must not write. In either case authenticated
cleanup may be impossible. The error reports this, local resources are released,
and **remote session expiration is unconfirmed**. No client can guarantee remote
cleanup after network loss; do not report it acknowledged.

The PTP service is never active during control requests: it is attached after
SETUP and stopped before TEARDOWN. This prevents control stalls from starving
PTP in the implemented diagnostic. It is not the concurrency design for future
playback, which must continue PTP while RECORD, feedback, anchors and media run.

The existing event worker is the only new diagnostic-owned worker. Its Drop
closes its socket and joins; PTP has no worker thread. Control/event connection
attempts are bounded by existing three-second timeouts. Control requests use
existing cancellation polling and bounded request budgets, cleanup uses 250 ms,
and observation is at most one second. File-backed selection and approval are
rechecked before control operations and each PTP datagram. UDP timing is not
cryptographically authenticated; IP/identity/sequence binding cannot prevent
on-path spoofing. No additional peer, group or proxy is ever authorized.

## Safe, manual hardware instructions

Do not run these during tests, startup, installation or review automation.
Do not change OwnTone, Autostream, receiver volume or system services.

1. Build normally and inspect `piair buffered-test --help` locally.
2. Use the existing explicit allowlist/select workflow for one receiver. For
   persistent authentication, enroll credentials separately only if needed and
   explicitly authorized. The diagnostic never pairs or changes selection.
3. `piair buffered-test device:<your-12-lowercase-hex-digits>` fails locally with
   the timing blocker. `--confirm-audio` also fails locally before discovery or
   receiver connection; it cannot force playback.
4. Only when authorized to contact that receiver, invoke
   `piair --config <your-config> buffered-test device:<your-id> --setup-check`
   in a terminal and read/confirm its prompt. It performs initial SETUP only,
   one second of PTP observation, then bounded cleanup. Ctrl+C cancels it. It
   sends no audio and never changes volume. Standard PTP port bind permission
   may be required; failure is reported, with no port fallback or service edits.
5. Record firmware/model, whether ClockID was present, PTP activity, failure
   stage and TEARDOWN outcome. Never record keys or decrypted secret payloads.
   This establishes no playback compatibility or audible result.

A future audio diagnostic must require separate explicit transmission
confirmation, a short bounded duration, a faded conservative low-level original
tone and cancellation, without changing receiver volume. Those instructions
cannot be used to transmit audio in this revision.

## Verification and remaining PR #43 dependencies

New tests use explicit loopback authorizations and encrypted mock control
connections. They test initial request order/token continuity; identity, features,
codec conflicts, timing address/clock/port rejection; terminal/CLI closed gates;
partial SETUP failure; cancellation; PTP shutdown; approval revocation; disconnect;
Drop; bounded stalled TEARDOWN; fresh owner identity; and impossible transitions
into playing/paused/prebuffering. Existing authentication, type103 ALAC/nonce,
TCP stall/backpressure, anchor acceptance/staleness, PTP loss and realtime/NTP
tests remain required. These are component and diagnostic tests, not a mock proof
of full playback. Full playable lifecycle fault injection is deferred with the
closed live gate. Hardware was not contacted.

PR #43 group/proxy playback must wait for a complete validated single-receiver
lifecycle, reliable adoption/clock recovery, continuously revocable media grants,
feedback/flow-control and pause/flush semantics. It will additionally need
separate per-member authorization, topology provenance, leader/proxy role
negotiation, shared clock/sample mapping and coordinated failure/teardown.
No group expansion or group readiness inference is introduced here.

## Prompt 44: transient authentication compatibility (2026-11-10)

Operator-supplied hardware evidence on main `9ad83d3` (PR #43): the approved,
locally selected Even-Olai HomePod, `device:3ae5c9a302ad`, endpoint
`192.168.1.33:7000`, AudioAccessory5,1 firmware 27.2, rejected
`piair -v buffered-test device:3ae5c9a302ad --setup-check` during M1/M2 with
HTTP 403. That path sent `POST /pair-setup HTTP/1.1`, `User-Agent: PiAir/0.1`,
and no preceding `/info`. It reached no SETUP, PTP or audio.

On that same receiver, the operator successfully ran:

```sh
piair -v authenticate device:3ae5c9a302ad --experimental-owntone-transient --control-check
```

The observed sequence was `GET /info RTSP/1.0` 200, transient pair-setup
M1/M2 and M3/M4 over RTSP 200, encrypted OPTIONS 200, and clean shutdown
without audio. This validates the combined authentication sequence for this
receiver; it does not isolate the individual cause of the failed path's 403.
No physical receiver was contacted during this implementation.

`buffered-test` now uses the existing OwnTone-reference RTSP profile as its
**transient default**, including its `/info` policy check, compatible
`AirPlay/999.0.0` User-Agent, SRP validation and same-socket encrypted channel.
It reuses `Wire::owntone_transient` and `pairing::transient`; no protocol logic
was copied from GPL sources or reimplemented. Apache-2.0 licensing is unchanged.
No additional experimental flag is required: this command already requires an
explicit buffered diagnostic, allowlisted/selected receiver, selected transient
method, fresh discovery checks and interactive operation confirmation. The CLI
announces the profile before confirmation. The `authenticate` and `stream-test`
defaults and experimental opt-ins are unchanged. There is no fallback.

Capabilities are read from the pinned authorization and revalidated; transient
SRP proves the connection, not a persistent receiver identity. It creates no
pairing credentials and leaves existing credentials untouched. As in the
existing profile, public sender identifiers may be initialized in protected
`receiver-state/client-identity.toml`; these are not receiver credentials.
The encrypted post-authentication `/info` identity/capability checks remain
mandatory. Cancellation, changed authorization, non-200 responses or invalid
SRP proofs drop the connection and cannot start PTP. SETUP/PTP and later
explicitly confirmed controls retain their existing implementation and cleanup.

Loopback regression tests exercise the buffered entry point through real SRP
and encrypted GET, exact RTSP request ordering/headers, 403 at `/info` and M1,
invalid proof, interrupted exchange, cancellation and revoked/changed selection.
They require closure without retry, fallback, audio or volume requests, and
compare all receiver-state files (including an existing credential) before/after.
Existing lifecycle tests cover mandatory authorization, post-authentication
SETUP/resource ordering and cleanup, and the permanently closed playback gate.

A subsequent Raspberry Pi test requires separate explicit authorization for the
exact receiver and interactive confirmation. Re-run the existing `buffered-test
... --setup-check` command and record authentication, encrypted `/info`, SETUP,
ClockID presence, standard-port PTP startup/activity and TEARDOWN outcome.
Only then consider the existing separately confirmed staged controls. No buffered
SETUP/PTP success is established by the authentication evidence or these mocks.
Clock adoption remains unverified; live buffered audio is disabled. Do not change
OwnTone, autostream, production configuration, volume or stored credentials.

## Prompt 45: authenticated format rejection (2026-11-10)

Operator evidence on `694d038` (merged PR #44): Even-Olai HomePod,
`device:3ae5c9a302ad`, AudioAccessory5,1 firmware 27.2 at
`192.168.1.33:7000`, completed initial GET /info, transient SRP M1/M2 and
M3/M4, and encrypted GET /info (all 200) using
`piair -v buffered-test device:3ae5c9a302ad --setup-check`. Validation then
reported `authenticated ALAC24/48000/stereo format unavailable or contradictory`.
No initial SETUP, PTP session or audio followed. OwnTone was restored successfully.
The actual authenticated format fields were not recorded, so the cause on this
receiver is still unknown. No receiver was contacted during this development.

### Evidence and interpretation

* **PiAir source facts:** `planning::Evidence::authenticated_info` reads
  `supportedFormats.bufferStream` as an unsigned 64-bit mask, converting set
  bit positions to format IDs. It reads
  `supportedAudioFormatsExtended.bufferStream` as an array of unsigned integer
  indices inside a dictionary. Either field may supply evidence alone; a
  malformed supplied field fails closed. Empty lists are valid evidence of no
  offered formats. At most 64 extended entries are accepted; duplicates now
  fail as malformed rather than being silently collapsed. Unknown unsigned IDs
  are retained as unknown, never selected as a codec.
* **Mapping evidence:** the independently maintained
  [airplay2-rs protocol ledger, format table](https://github.com/lmcgartland/airplay2-rs/blob/master/AIRPLAY_2_SPEC.md)
  records bit21 / `0x200000` as ALAC/48000/24/2. This is reverse-engineered
  documentation, not an Apple normative specification. It corroborates PiAir's
  existing mapping; no evidence warrants changing it. `buffered::select_format`
  selects the implemented ALAC24 variant; `Prepared::setup_offer` emits
  `audioFormatIndex=21` and `audioFormat=2097152`. Index 21 is a bit position,
  not the twenty-second element of an advertised list. `profile.rs` has a
  separate realtime mask check; it does not supply buffered capability evidence.
* **Observed representation:** a
  [public pyatv AppleTV14,1 capture](https://github.com/postlund/pyatv/issues/2642)
  contains both dictionaries, an extended bufferStream integer array with IDs
  above 63 (including 72), and a legacy 64-bit bufferStream mask. This is
  evidence of a representation used by one Apple TV, not this HomePod's
  authenticated response or a universal receiver contract. The independently
  constructed regression uses only bit21 and index72, without private capture
  fields. The old exact-set comparison necessarily rejected any such high IDs.
  Reconciliation now compares the legacy mask with only the extended indices
  below 64. Higher IDs remain unknown evidence; disagreement within 0..63 still
  rejects. No codec fallback or model inference is introduced.
* **Receiver differences:** the pinned
  [openairplay receiver](https://github.com/openairplay/airplay2-receiver/blob/6c343d3679ddb561c61566985acaaf587d0a3bd3/ap2-receiver.py)
  leaves the legacy format dictionary commented out in its info construction.
  A receiver implementation can therefore omit tables; its ability to decode
  audio is not authenticated format evidence. No GPL OwnTone code was copied,
  translated or incorporated; the correction is set projection over PiAir's
  existing typed representation.

**Hypotheses, not conclusions:** the HomePod might omit a field, supply a different
shape, disagree within the legacy range, or advertise higher unknown indices.
The public Apple TV dump also displays a negative legacy integer; PiAir still
rejects negative integers rather than guessing signed-mask reinterpretation.
No broader signedness change is justified by this HomePod evidence. The
64-entry bound may reject a future larger legitimate table; it remains explicit
and fail-closed pending evidence. Neither HomePod branding nor authentication
success proves index21 support.

### Diagnostics and next Raspberry Pi test

A format rejection now appends a fixed-field summary to the error (visible with
`-v`): parent and bufferStream plist types (`missing` if absent), array entry
count or mask population count, numeric unsigned IDs, per-field `required21`,
array truncation, malformed-element/duplicate indicators, and a final `missing`, `malformed`, `contradictory`,
`unsupported` or `supported` classification. Final `required21` means validated
support; per-field presence can be true even when reconciliation fails. Array
IDs are limited to the first 64 entries; count is the full entry count, and
invalid elements are omitted from the ID summary but still invalidate parsing.
Negative masks report `invalid-unsigned-mask`. No arbitrary strings, dictionary
keys, data bytes, full plists, secrets or unrelated private fields are formatted.
Nested malformed values are not traversed. Existing bounded RTSP response parsing
remains unchanged. The complete two-field diagnostic is bounded under 4096 bytes.

The next manually authorized Pi test is the same command above, with its existing
interactive confirmation and receiver authorization. Record authentication and
encrypted GET status plus the sanitized format failure summary. If validation
passes, this command can proceed to its existing initial SETUP/PTP diagnostic;
record that stage and cleanup outcome too. Do not add staged-control flags or
`--confirm-audio`. Do not dump decrypted /info or change OwnTone, autostream,
credentials or volume. This development task does not authorize an automated
physical test. Playback stays disabled. The next summary should establish which
field/type/IDs were observed and whether the demonstrated reconciliation fix is
relevant; if a new shape or signed mask appears, investigate independently before
changing acceptance policy.

Validation for Prompt 45: `cargo fmt --check`,
`cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`,
`cargo build --release --locked`, and `git diff --check` passed.
Final tests: 278 unit tests and 23 integration tests, zero failures/ignored.
An initial sandboxed run failed because loopback socket binds were denied;
reruns with loopback access passed. Rust and ALSA development files were installed
under the development workspace; no dependency or lockfile change was needed.
All receiver traffic in validation used local mocks.
