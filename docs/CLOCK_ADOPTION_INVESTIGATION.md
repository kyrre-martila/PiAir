# Clock adoption investigation after PR #49

Current receiver discovery, selection and command usage is documented in
[Receiver diagnostics](RECEIVER_DIAGNOSTICS.md). Receiver-specific observations
and authorizations below are historical evidence, not current CLI restrictions.
Examples use placeholders; verify the canonical ID independently.

Scope: main `83862dd`, reported deployed Stua HomePod firmware 27.2.
The operator reports authenticated initial SETUP, accepted Delay_Req sequences
1–8, Delay_Resp transmission, PeerObserved, and encrypted TEARDOWN success.
These are supplied observations, not captures independently reproduced here.
No receiver was contacted during this investigation. Audio remains blocked.

## Evidence and decision

The exact OwnTone Mini reference is
[`e9b058448b7d921ed048790b45614685fd13802b`](https://github.com/lo-tech-systems/owntone-mini/tree/e9b058448b7d921ed048790b45614685fd13802b).
References were read for protocol facts only; no source was copied, translated,
linked or incorporated. In particular, GPL `src/ptpd.c` and
`src/outputs/airplay.c` supplied no implementation skeleton.

| Question | Verified reference behavior | Inference / remaining unknown |
| --- | --- | --- |
| Inbound Sync / Follow_Up | `src/libairptp/src/ptp_msg_handle.c` L523–553 parses their headers/timestamps and logs them; neither handler elects a GM, measures receiver lock, nor checks the Apple Follow_Up TLV. | Their presence is compatible with the reference's fixed GM operation. It does not establish that a receiver has selected either clock. PiAir's event Sync rejection is an intentional role boundary, not evidence of malformed HomePod behavior. |
| Apple subtype 4 | Same file L384–429 emits OUI 00:0d:93, subtype 4, length16, own clock ID and zero padding. The independent Apache-2.0 [airplay-cli reference](https://github.com/music-assistant/airplay-cli/blob/0a58c239d5fb3cf8927b9707a71c6fe897266b6a/src/ap2_ptp.c#L475) emits the same layout. | Verified for these senders' emitted packets only. Neither proves that all receiver-generated subtype4 values must equal header source identity. Selected GM identity, timeline identifier, or another private meaning remains unknown for Stua's conflicting value. |
| Clock quality | OwnTone L298–329 sends class6, accuracy0x21, variance0x436a, GPS source0x20 and PTP timescale, with comments about strict receivers. | Comments about faster lock are an empirical implementation claim, not a lock measurement or justification to misrepresent PiAir's oscillator. Keep truthful class248/unknown/internal oscillator and existing priorities. |
| Signaling | OwnTone L333–370 emits private Apple subtypes1/5 with an unexplained marker. L685–850 logs/ignores some TLVs and handles private peer registration. Independent airplay-cli L565–618 responds to REQUEST_UNICAST_TRANSMISSION TLVs with grants. | Unicast flag alone is not a negotiation transcript or adoption signal. There is no supplied Stua request/grant trace showing a missing grant caused this failure. Private Apple signaling and standard unicast request/grant have different meanings. Add neither speculative grants nor private markers. |
| Daemon evidence | `src/ptpd.c` L78–93 exposes peer age, delay counts/timestamps and signaling counts. L99–161 can reuse a host daemon; L167–186 reads local CLOCK_MONOTONIC for network time. | These are local liveness/timeline facts, not an authenticated remote servo measurement. PiAir retains session-owned sockets and per-datagram authorization checks. |
| Control and anchor | `src/outputs/airplay.c` L6592–6625 orders session SETUP, RECORD, SETPEERS, command tables, stream SETUP and audioMode. L5590–5706 describes delay statistics and checks RTSP plus in-band anchor status. L7962–7985 delays anchor release by peer age; rejection handling includes retries. | Age/counts and accepted anchor are heuristics/control outcomes, not a continuous receiver adoption attestation. PiAir already implements the requested stages and paused anchor; do not reproduce reference retries, full command claims, volume, or playable anchors. |

IEEE PTP separates sourcePortIdentity (transmitting clock plus port) from the
Announce grandmasterIdentity (selected GM, which can differ in a boundary-clock
topology). The source clock, advertised GM, authenticated SETUP ClockID, and
private Apple subtype4 value must therefore be compared as separate fields.
IEEE identities and Announce quality are unauthenticated UDP claims. IEEE 1588
and 802.1AS wire layouts do not define Apple's private identity semantics.
See the independent standard layout references in [PTP_TIMING.md](PTP_TIMING.md).
No BMCA or follower implementation is justified by these observations.

The reported `conflicting Apple clock TLV` does **not** identify which validation
failed: PiAir currently combines clock equality and two-byte padding in that
error. Without the datagram we cannot establish the value, padding, length,
source port, relation to an Announce GM, or whether Sync/Follow_Up sequences
matched. Calling it a receiver bug or a parser bug would exceed the evidence.
PiAir's equality check remains a conservative supported subset, not a proven
universal Apple rule. No TLV acceptance is relaxed in this PR.

## Adoption criterion remains unavailable

Keep these claims separate:

- PTP liveness: fresh forward-sequence delay requests from the connected,
  authorized receiver IP and the control-supplied ClockID. UDP itself is not
  authenticated. Wrong identities, malformed packets and replays cannot extend
  this freshness, nor can general traffic. Existing three-second loss bound
  continues during control requests.
- RTSP acceptance: authenticated response, matching CSeq, RTSP200 and validated
  body semantics. A nonzero plist status rejects even an RTSP200 response.
- Anchor acceptance: the above for the fresh, same-generation **paused** mapping.
  It says that the request was accepted, not that the timeline is in use.
- Receiver clock lock/adoption: **unverified**. None of the studied evidence
  supplies an authenticated, session-bound, fresh and renewable receiver report
  of selected sender timeline plus offset/servo state and discontinuity/loss
  semantics. Echoed IDs, Announce selection, private TLV agreement, elapsed
  time, delay counts, and PiAir's local estimator do not fill that gap.

The next defensible step is a separately authorized, bounded Stua-only evidence
collection using existing diagnostics. Correlate identities, both PTP channels,
control outcomes and timing loss. A receiver-originated GM claim could narrow
role/identity hypotheses but still cannot by itself authorize media. Before any
future adoption gate, independently establish the report's semantics, origin,
session/timeline binding, freshness and revocation behavior. No such gate is
added here; media_permitted remains false and no rate=1 path is exposed.

## Diagnostic changes

The CLI now prints one bounded summary after successful completion or a failed
start/observation/control/cleanup. It retains the last semantically accepted
operation, attempted operation, PTP state snapshot before cleanup, adoption
status, TEARDOWN outcome and zero audio bytes. Intermediate SETPEERS, command
registration and stream SETUP are distinguished from the aggregate stream stage.
The original error is propagated; validated signed in-band timing-control status
is included as a number, without printing receiver text or plist values.

A snapshot is historical, not continuous post-session health. Service shutdown
can represent cancellation, revocation or timing failure; inspect the propagated
error to distinguish them. A failed worker can suppress control acceptance even
if a reply arrived. When control is invalidated/revoked, cleanup remains bounded
and remote expiry is unconfirmed. Diagnostic fields are fixed vocabulary and
state enums; they contain no IP, clock ID, session token, keys or payloads.
No control ordering, confirmation, authorization, packet policy or retry changes.

## Safe manual Stua procedure (not executed)

Use only the already approved standalone Stua device (reported firmware27.2).
Resolve its identifier and address from the operator's private local configuration;
replace `<Stua-id>` below with that identifier. Confirm local selection still
resolves to that device and current authorization is valid. Stop on changed identity/address, group/stereo topology, bind conflicts,
revocation or any failed stage. Do not modify Pi services, grant privileges,
change volume, or automatically retry. Read local `--help` first.

Run each command once, stopping to review its result before proceeding:

```text
piair --config <config> buffered-test 'device:<canonical-id>' --setup-check
piair --config <config> buffered-test 'device:<canonical-id>' --control-check record
piair --config <config> buffered-test 'device:<canonical-id>' --control-check stream
piair --config <config> buffered-test 'device:<canonical-id>' --control-check anchor
```

Honor every existing terminal confirmation; all confirmations precede timing.
Each invocation creates a fresh authenticated session, observes timing for at
most one second, executes only the confirmed ordered stages, and performs
bounded TEARDOWN. Anchor uses rate0 only. No media connection, audio bytes or
volume requests occur. Ctrl+C cancels. Do not use `--confirm-audio` as a bypass.

Record last accepted/attempted operation, PTP snapshot, sanitized error including
in-band status if present, and acknowledged/unacknowledged TEARDOWN. All stages
must still say adoption unverified, including an accepted paused anchor.
For a separately approved capture, restrict collection to this receiver and the
brief session; retain the original privately. Do not publish decrypted control,
keys, tokens or full identity-bearing packets. Produce a redacted correlation
table of event/general type, sequence, source clock/port aliases, Announce GM
alias and quality, subtype4 ID alias/padding validity, signaling type/target and
request/grant duration, delay continuity and control-stage timestamps. Preserve
whether IDs agree without exposing their raw values. Collect enough to test the
hypotheses above, not an unbounded recording. Do not induce network loss on the
real receiver; loss/revocation behavior is tested with local mocks.

## Local validation

Rust/Cargo 1.99.1, x86_64 Linux, locked dependencies. Toolchain and official
Debian ALSA development files were installed/extracted under `/workspace`;
no system service, receiver or repository dependency was changed. Loopback
socket tests ran with the execution sandbox's network restriction lifted.

- `cargo fmt --check`: passed.
- `cargo clippy --locked --all-targets -- -D warnings`: passed (including final test assertions).
- `cargo build --locked --release`: passed.
- `cargo test --locked receiver::ptp::tests`: 32 passed.
- `cargo test --locked receiver::lifecycle`: 21 passed.
- `cargo test --locked`: 299 unit tests, 6 allowlist integration tests and
  23 CLI integration tests passed; zero failures.
- `git diff --check`: passed; Cargo.lock unchanged.

Tests extend the existing encrypted staged-session fixtures to check successful
record/stream/paused-anchor summaries, every control-stage in-band rejection,
failed TEARDOWN, authorization revocation, cancellation and timing loss. The
PTP hypothesis fixture verifies inbound Sync/different-identity Apple Follow_Up,
malformation, replay and wrong-clock rejection do not extend liveness or grant
media. It does not claim to reconstruct unreported Stua bytes.

No reproducible regression or flaky failure occurred in this run. The known
remote-TEARDOWN mock scheduling failure documented in
[PR #49's validation](PTP_DELAY_REQ_COMPATIBILITY.md#recorded-local-results)
and [PR #48's validation](PR48_VALIDATION.md) passed here. Those historical
failures are not new failures of this change. No receiver verification occurred.
