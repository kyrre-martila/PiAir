# HomePod Delay_Req compatibility

Current receiver discovery, selection and command usage is documented in
[Receiver diagnostics](RECEIVER_DIAGNOSTICS.md). Receiver-specific observations
and authorizations below are historical evidence, not current CLI restrictions.
Examples use placeholders; verify the canonical ID independently.

## Evidence and root cause

Reported hardware evidence (not a test run performed for this change): the
explicitly authorized **Example HomePod, 192.0.2.119, firmware 27.2** sends
PTPv2 Delay_Req on UDP 319 after authenticated SETUP, with controlField=0
and sequences 1–9. PiAir sends Sync/Follow_Up, but no Delay_Resp is observed
and its state remains Synchronizing. Receiver clock adoption is unverified.

The deterministic `stua_delay_req` fixture is hand assembled, not encoder
output or a claimed complete capture. It reconstructs those reported fields
using transportSpecific=1, version=2, domain=0, length=44, unicast flags,
zero correction/reserved bytes and a valid zero origin timestamp. Clock ID
1122334455667788 and port 8005 are synthetic. Actual unreported fields must
still pass validation; this test does not prove every captured byte was valid.

Against unchanged main (02f5ae86b86396d6497b525d07c065c19cb4bf3e),
`cargo test --locked stua_zero_control_delay_req_raw_regression` fails with
`invalid PTP control field`. Decode rejects byte 32 before the Grandmaster's
existing DelayResp generation runs. Changing only that byte to 1 passes;
after the fix both decode identically and use the same replay slot.
Thus the reported control value is a concrete rejection cause, without
claiming hardware validation of the resulting exchange.

## Standards and reference comparison

IEEE 1588-2008 section 13.3.2.14 defines the legacy controlField values:
Sync=0, Delay_Req=1, Follow_Up=2, Delay_Resp=3, Management=4, others=5.
The separate messageType identifies the PTPv2 message. Delay_Req is an event
message; Delay_Resp is a general message and carries the request's sequence,
requesting sourcePortIdentity and the master's receive timestamp. Zero for
Delay_Req differs from the standard legacy encoding; treat it as an explicit
receiver compatibility exception, not as a reason to discard other checks.

Reviewed the pinned [OwnTone Mini ptp_msg_handle.c](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/libairptp/src/ptp_msg_handle.c):

- `header_init` emits controlField=0 for every message.
- `header_read` copies the header without checking controlField.
- `ptp_msg_handle` dispatches on the low four messageType bits.
- `delay_msg_handle` checks minimum size, builds a response with the request's
  sequence and source port identity, and sends through the general service.

PiAir does not adopt that broad acceptance or peer-routing policy. The only
new parser exception is zero controlField for **DelayReq**. Existing zero
exceptions for FollowUp/DelayResp/Announce remain unchanged; Pdelay and
Signaling still require 5. Encoding retains standard legacy values.
No reference source code was copied, translated or linked; comparison is of
protocol behavior only, retaining PiAir's Apache-2.0 implementation.

## Bounds and verification

ClockID, pinned peer port identity, sequence/replay, correction, datagram and
body length, timestamp, flags, source socket and live authorization checks
remain in place. Malformed requests cannot consume a sequence or pin a peer.
No sockets, receiver contacts, retries, clock election or playback grants were
added. PeerObserved means delay traffic was observed, not remote servo lock;
`media_permitted()` remains false in every state.

Tests cover sequences 1–9 with both control encodings; all 256 control values
for each supported message type; exact deterministic receive timestamps;
identity/sequence/correction/flags and malformed headers; loopback event
receive to general-channel response; wrong-channel rejection; replay across
encodings; and revocation before a queued response. Existing source filtering,
ClockID, authorization and playback-gate tests also run.

Debug (`-v`) target `piair::ptp` logs at most **8 rejected datagrams and
8 successful Delay_Req/Delay_Resp exchanges per Service lifetime**, with
independent budgets. Rejects include channel, received length, optional numeric
messageType/control and a fixed validation error. Successful exchanges log
sequence/general routing only after a successful send. No packet dump, address,
clock/port identity, secret, plist, key or media content is logged. Floods cannot
consume the success budget. Diagnostics are silent at INFO.

## Manual Example-only verification (not performed)

1. In a separate checkout on the Pi, build this PR and read local help. Retain
   the existing explicit selection/allowlist/authentication for Example only.
   Verify the selected receiver still resolves to 192.0.2.119 and is firmware
   27.2. Stop on identity, ClockID or address conflicts; do not infer a missing
   ClockID from UDP traffic or change any service/privilege configuration.
2. Run the existing bounded initial-SETUP-only diagnostic with debug logging:
   `piair -v --config <config> buffered-test 'device:<canonical-id>' --setup-check`.
   Complete its existing terminal confirmation before it connects. Do not use
   control-check, RECORD, stream, anchor, playback, volume or group modes.
3. Confirm authenticated SETUP supplies the expected ClockID, then observe a
   zero-control Delay_Req on event UDP 319 followed by a matching Delay_Resp
   on general UDP 320. Compare sequence, requesting port identity and receive
   timestamp locally if an already authorized capture is available; do not
   publish raw captures or identifiers. Expect PeerObserved only. The short
   setup window may show fewer than nine requests; do not add retries.
4. Check normal bounded TEARDOWN/socket cleanup (or cancel with Ctrl+C). Record
   sanitized firmware, stage, rejection reason or exchange sequence and teardown
   outcome. A response or PeerObserved does not prove clock adoption or audio
   readiness. No RECORD/media/volume test is authorized by this procedure.

No hardware test, new receiver contact or service change was performed for this
PR. Build/test results are recorded below and in the PR description.

## Recorded local results

Rust/Cargo 1.99.1, x86_64 Linux, locked dependencies. Rustfmt/Clippy were
installed in the workspace. Builds used the installed ALSA runtime with a
workspace-local linker symlink/pkg-config description (Rust uses pre-generated
bindings); no system package, service or repository dependency was changed.
Socket tests ran with local loopback access. Initial sandbox-only socket errors
were environment restrictions and are not counted as test regressions.

| Check | Result |
| --- | --- |
| Raw regression against unchanged parser | Failed: `invalid PTP control field` (expected red) |
| `cargo fmt --check` | Pass |
| `cargo clippy --all-targets --locked -- -D warnings` | Pass |
| `cargo build --release --locked` | Pass |
| `cargo test --locked receiver::ptp::tests` | 31 passed |
| `cargo test --locked` on changed tree | 297 unit tests passed, 1 failed |
| `cargo test --locked --test allowlist --test cli` | All 29 integration tests passed |
| Clean base `cargo test --locked --offline` in separate target directory | 293 unit tests and all 29 integration tests passed |
| Exact isolated rerun of the changed-tree failure | 1 passed |

The full changed-tree run failed only
`receiver::tests::stream_tests::record_tests::both_stream_modes_report_failed_remote_teardown_and_release_locally`
at `src/receiver/record_tests.rs:849`, `TeardownReject`, left=295/right=376.
It is an existing NTP/mock media scheduling test, unrelated to the PTP receive
path. The first changed-tree suite ran alongside compilation; it is **not a
green full-suite result**. The unchanged-base suite passed, and the changed-tree
test passed alone with no competing build/test process:

```text
cargo test --locked receiver::tests::stream_tests::record_tests::both_stream_modes_report_failed_remote_teardown_and_release_locally -- --exact
```

This same test/assertion has documented main failures in
[PR #48 validation](PR48_VALIDATION.md) (left=5/right=376). Thus the observed
failure matches a documented pre-existing intermittent test, rather than a
new PTP test failure; it was not reproduced in this session's clean-base run.
No assertion, timeout or playback behavior was changed. No production
regression was demonstrated. A shared-target baseline attempt was discarded
because it reused the changed binary; only the isolated 293-test base run is
reported above.
