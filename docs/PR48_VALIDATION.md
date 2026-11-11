# PR #48 follow-up validation

Final outcome: formatting, Clippy and release build pass. The exact serial test
command fails with 282 unit tests passed and 11 failed; every final failure is
demonstrated on current main. The complete first PR run passes all 29 integration
tests and the final run passes every new timing-peer test. This follow-up merges
main and records evidence; it changes no production code or test assertions.

The follow-up merges main `61893e05c118a40a90111df3aa1e3d8475e481f3`
into PR head `36703147a702ac3ad40b0e0bc3ce47aa087b2c4b`. Main adds the
Pachelbel test asset and its license; this follow-up does not alter either file.
Their Git blob IDs are identical in main and the updated PR tree:

| File under `tests/assets/audio/` | Git blob |
| --- | --- |
| `Pachelbel_Canon_audio_test.wav` | `fac46f10baaa5255bfbd60707c968913b7e84beb` |
| `LICENSE.md` | `2ca5c7a4850197ccec161923cfdcb7869b4d5270` |

## Conditions and comparison

Tests run as the ordinary user outside the command sandbox, permitting local
TCP/UDP loopback fixtures. No real receivers or hardware are contacted. The
existing suite exercises simulated receiver protocols only. Rust and Cargo are
1.99.1 on x86_64 Linux, with the locked dependency graph and default unoptimized
test profile. `RUST_TEST_THREADS=1` serializes each suite. Compilation and other
test processes do not compete with the test run. The same toolchain, dependency
cache, ALSA development package, environment and execution permissions are used
for PR and main in separate worktrees, sequentially.

The first session's concurrent/incomplete runs are not evidence of a baseline
failure. Its isolated feedback test reported:

```text
receiver::tests::stream_tests::feedback_stream_identity_mismatch_fails_the_active_session
assertion failed: session.failure().unwrap().contains("stream identifier")
```

That historical result is compared with fresh complete runs below; no assertion,
timeout, scheduler limit or production behavior is weakened to obtain a pass.

## Trust review

- `InitialSetup::parse` still requires the authenticated SETUP ClockID, rejects
  zero/all-ones/local ClockIDs, and rejects additional timing peer identities.
  A valid advertised IP alone cannot establish PTP liveness or clock adoption.
- Selection remains numeric-only, bounded, ordered and deduplicated. The RTSP IP
  is preferred when advertised and compatible. No DNS lookup, inferred address,
  peer retry after timeout, or unadvertised destination is introduced.
- `Wire::start_ptp` changes only the timing destination of the session capability;
  its original receiver authorization/selection and per-operation revocation
  checks remain intact. The service validates both connected socket peer IPs.
- Both UDP sockets bind the control local IP and connect to the selected peer
  on standard event/general ports. Connected UDP filters other source IPs and
  ports; PTP decoding, authenticated ClockID, port identity, sequence and message
  checks remain in place. General traffic cannot refresh liveness or add peers.
- The route-source check runs before binding the PTP sockets. It is not a device
  binding guarantee and cannot track later route changes. Scoped IPv6 link-local,
  cross-family and multihomed timing remain unsupported as documented in
  [authenticated-ptp-peers.md](authenticated-ptp-peers.md).
- Setup attempts are recorded before sending. Address validation and attachment
  failures still enter bounded encrypted TEARDOWN and release sockets/workers.
  Cancellation, clock loss, authorization revocation and invalidated control
  retain their existing cleanup policies. No timing evidence grants media.

The follow-up introduces no production protocol change, new RECORD, media,
volume operation, persistent pairing, or relaxed validation.

## Complete serial PR run

Command: `RUST_TEST_THREADS=1 cargo test --locked --no-fail-fast`.
The additional `--no-fail-fast` runs integration targets even when the unit target
fails. Exit 101: **287 unit tests passed, 6 failed; all 6 allowlist and 23 CLI
integration tests passed**. All five added timing-peer regression tests passed.
The exact failed test names and assertion/causal message excerpts follow;
variable thread IDs, timestamps and verbose mock RTSP traces are omitted.

```text
receiver::tests::stream_tests::record_tests::stua_persistent_failures_teardown_and_release_without_retry
src/receiver/record_tests.rs:849
assertion `left == right` failed: TeardownReject
  left: 25
 right: 376

receiver::tests::stream_tests::record_tests::stua_persistent_playback_decrypts_and_decodes_fixed_faded_tone
src/receiver/record_tests.rs:820
assertion `left == right` failed: Good: Err(bounded TEARDOWN also failed: RTSP method=TEARDOWN: encrypted RTSP method=TEARDOWN phase=encrypted TEARDOWN elapsed_ms=262 response_bytes=0 authenticated_frames=0: response read/validation: response headers: receiver operation timed out: receiver read timeout
Caused by:
    audio scheduler stalled; no uncontrolled catch-up)
  left: false
 right: true

receiver::tests::stream_tests::record_tests::stua_selected_volume_reaches_encrypted_receiver_without_changing_tone
src/receiver/record_tests.rs:820
assertion `left == right` failed: Good: Err(bounded TEARDOWN also failed: RTSP method=TEARDOWN: encrypted RTSP method=TEARDOWN phase=encrypted TEARDOWN elapsed_ms=259 response_bytes=0 authenticated_frames=0: response read/validation: response headers: receiver operation timed out: receiver read timeout
Caused by:
    audio scheduler stalled; no uncontrolled catch-up)
  left: false
 right: true

receiver::tests::stream_tests::request_failures_attribute_each_negotiation_stage_and_close_channels
src/receiver/stream_tests.rs:938
called `Result::unwrap()` on an `Err` value: pair-verify did not complete
Caused by:
    0: /pair-verify M4 outer TLV exchange
    1: response headers
    2: receiver read timeout

receiver::tests::stream_tests::transient_session_negotiates_encrypted_audio_and_acknowledges_teardown
src/receiver/stream_tests.rs:615 (receiver thread)
assertion failed: sync_count.load(Ordering::Relaxed) >= 3
src/receiver/stream_tests.rs:1008 (test thread)
called `Result::unwrap()` on an `Err` value: audio scheduler stalled; no uncontrolled catch-up

receiver::wire::worker_tests::setup_response_progress_distinguishes_silence_partial_frames_and_closure
src/receiver/wire.rs:1925
assertion `left == right` failed
  left: false
 right: true
```

## Current main comparison

Main was rebuilt after removing the package artifacts from the shared target
directory. A first comparison attempt reused the 293-test PR binary and was
discarded. The valid main binary has 288 tests and no PR-only test names.

Command: `RUST_TEST_THREADS=1 cargo test --locked --no-fail-fast --target-dir /workspace/PiAir/target`, from the clean main worktree.
Exit 101: **272 unit tests passed, 16 failed; all 29 integration tests passed**.
The following are exact main failure excerpts, with verbose mock traces and
variable thread metadata omitted.

```text
receiver::tests::setup_failures_identify_stage_and_preserve_partial_enrollment_warning
src/receiver/tests.rs:50:35
called `Result::unwrap()` on an `Err` value: Os { code: 11, kind: WouldBlock, message: "Resource temporarily unavailable" }
src/receiver/tests.rs:509:9
missing fixed stage context

receiver::tests::stream_tests::feedback_stream_identity_mismatch_fails_the_active_session
src/receiver/stream_tests.rs:808:5
assertion failed: session.failure().unwrap().contains("stream identifier")

receiver::tests::stream_tests::full_authenticated_realtime_session_encodes_encrypts_synchronizes_and_tears_down
src/receiver/stream_tests.rs:615:9
assertion failed: sync_count.load(Ordering::Relaxed) >= 3
src/receiver/stream_tests.rs:647:34
called `Result::unwrap()` on an `Err` value: audio scheduler stalled; no uncontrolled catch-up

receiver::tests::stream_tests::initial_ntp_response_contract
src/receiver/stream_tests.rs:24:36
called `Result::unwrap()` on an `Err` value: Error { kind: UnexpectedEof, message: "failed to fill whole buffer" }
src/receiver/stream_tests.rs:1126:19
called `Result::unwrap()` on an `Err` value: Any { .. }

receiver::tests::stream_tests::owntone_setup_same_socket_counters_validation_and_cleanup
src/receiver/stream_tests.rs:1563:10
called `Result::unwrap()` on an `Err` value: transient authentication M4
Caused by:
    0: /pair-setup M4 outer TLV exchange
    1: response headers
    2: receiver read timeout

receiver::tests::stream_tests::record_tests::both_stream_modes_report_failed_remote_teardown_and_release_locally
src/receiver/tests.rs:50:35
called `Result::unwrap()` on an `Err` value: Os { code: 11, kind: WouldBlock, message: "Resource temporarily unavailable" }
src/receiver/record_tests.rs:792:14
called `Result::unwrap()` on an `Err` value: transient authentication M4
Caused by:
    0: /pair-setup M4 outer TLV exchange
    1: Broken pipe (os error 32)

receiver::tests::stream_tests::record_tests::new_modes_preserve_event_health_early_revocation_and_precontact_capability_gates
src/receiver/tests.rs:50:35
called `Result::unwrap()` on an `Err` value: Os { code: 11, kind: WouldBlock, message: "Resource temporarily unavailable" }
src/receiver/record_tests.rs:792:14
called `Result::unwrap()` on an `Err` value: transient authentication M4
Caused by:
    0: /pair-setup M4 outer TLV exchange
    1: Broken pipe (os error 32)

receiver::tests::stream_tests::record_tests::playback_check_receiver_decrypts_decodes_and_validates_bounded_pacing
src/receiver/record_tests.rs:820:9
assertion `left == right` failed: Good: Err(bounded TEARDOWN also failed: RTSP method=TEARDOWN: encrypted RTSP method=TEARDOWN phase=encrypted TEARDOWN elapsed_ms=262 response_bytes=0 authenticated_frames=0: response read/validation: response headers: receiver operation timed out: receiver read timeout
Caused by:
    audio scheduler stalled; no uncontrolled catch-up)
  left: false
 right: true

receiver::tests::stream_tests::record_tests::playback_stops_promptly_on_media_cancellation_revocation_and_resend_request
src/receiver/tests.rs:50:35
called `Result::unwrap()` on an `Err` value: Os { code: 11, kind: WouldBlock, message: "Resource temporarily unavailable" }
src/receiver/record_tests.rs:792:14
called `Result::unwrap()` on an `Err` value: transient authentication M4
Caused by:
    0: /pair-setup M4 outer TLV exchange
    1: Broken pipe (os error 32)

receiver::tests::stream_tests::record_tests::record_contract_same_connection_counters_events_timing_and_cleanup
src/receiver/tests.rs:50:35
called `Result::unwrap()` on an `Err` value: Os { code: 11, kind: WouldBlock, message: "Resource temporarily unavailable" }
src/receiver/record_tests.rs:799:14
called `Result::unwrap()` on an `Err` value: transient authentication M4
Caused by:
    0: /pair-setup M4 outer TLV exchange
    1: Connection reset by peer (os error 104)

receiver::tests::stream_tests::record_tests::stream_and_playback_fail_closed_on_stream_transport_schema_cancellation_and_revocation
src/receiver/record_tests.rs:792:14
called `Result::unwrap()` on an `Err` value: transient authentication M4
Caused by:
    0: /pair-setup M4 outer TLV exchange
    1: response headers
    2: receiver read timeout

receiver::tests::stream_tests::record_tests::stua_persistent_failures_teardown_and_release_without_retry
src/receiver/record_tests.rs:642:74
called `Result::unwrap()` on an `Err` value: Os { code: 11, kind: WouldBlock, message: "Resource temporarily unavailable" }
src/receiver/record_tests.rs:849:9
assertion `left == right` failed: TeardownReject:
  left: 5
 right: 376

receiver::tests::stream_tests::record_tests::stua_persistent_playback_decrypts_and_decodes_fixed_faded_tone
src/receiver/record_tests.rs:820:9
assertion `left == right` failed: Good: Err(bounded TEARDOWN also failed: RTSP method=TEARDOWN: encrypted RTSP method=TEARDOWN phase=encrypted TEARDOWN elapsed_ms=260 response_bytes=0 authenticated_frames=0: response read/validation: response headers: receiver operation timed out: receiver read timeout
Caused by:
    audio scheduler stalled; no uncontrolled catch-up)
  left: false
 right: true

receiver::tests::stream_tests::record_tests::stua_selected_volume_reaches_encrypted_receiver_without_changing_tone
src/receiver/record_tests.rs:820:9
assertion `left == right` failed: Good: Err(bounded TEARDOWN also failed: RTSP method=TEARDOWN: encrypted RTSP method=TEARDOWN phase=encrypted TEARDOWN elapsed_ms=255 response_bytes=0 authenticated_frames=0: response read/validation: response headers: receiver operation timed out: receiver read timeout
Caused by:
    audio scheduler stalled; no uncontrolled catch-up)
  left: false
 right: true

receiver::tests::stream_tests::request_failures_attribute_each_negotiation_stage_and_close_channels
src/receiver/stream_tests.rs:938:55
called `Result::unwrap()` on an `Err` value: pair-verify did not complete
Caused by:
    0: /pair-verify M4 outer TLV exchange
    1: response headers
    2: receiver operation timed out
    3: receiver read timeout

receiver::tests::stream_tests::transient_session_negotiates_encrypted_audio_and_acknowledges_teardown
src/receiver/stream_tests.rs:615:9
assertion failed: sync_count.load(Ordering::Relaxed) >= 3
src/receiver/stream_tests.rs:1008:30
called `Result::unwrap()` on an `Err` value: audio scheduler stalled; no uncontrolled catch-up

```

## Comparison conclusion

Five of the initial PR run's six failed tests also failed on rebuilt current
main: the three `stua_*` tests, `request_failures_attribute_each_negotiation_stage_and_close_channels`,
and `transient_session_negotiates_encrypted_audio_and_acknowledges_teardown`.
The main excerpts above demonstrate those failures; they are not inferred from
similar names or from the discarded cached run. Packet counts and elapsed times
vary, while the scheduler-stall/read-timeout causes recur. Main also reproduces
the historical feedback-identity assertion quoted near the start of this note.

The sixth test, `setup_response_progress_distinguishes_silence_partial_frames_and_closure`,
passed on main in the complete run and in a separate exact isolated run of the
rebuilt main binary (1 passed, 287 filtered out). Its initial PR failure is **not**
classified as a demonstrated main failure. Its request/parser implementation and
assertions are unchanged by the PR; the final PR rerun is recorded below.

No new timing-peer test failed in the first complete PR run. No production
regression has been demonstrated by this comparison, and no unrelated protocol,
scheduler, timeout or test-assertion changes are made. The host's ability to bind
loopback sockets is established; its ability to meet all existing short realtime
and diagnostic deadlines is not established by these runs. The full suite must
not be reported green unless a complete run actually passes.

## Final required test command

After rebuilding the PR package (confirmed 293-test binary),
`RUST_TEST_THREADS=1 cargo test --locked` exits 101: **282 unit tests passed,
11 failed**. This default invocation stops before integration targets after the
unit target fails; all 29 integration tests passed in the earlier complete
`--no-fail-fast` run of the same source tree. All five new timing-peer tests and
the existing lifecycle/PTP cleanup, ClockID and revocation tests pass again.
The initially failing setup-response classification test passes on this rerun.

Ten final failures are also present in the valid main full-suite output. The
remaining final failure, `stalled_control_has_an_absolute_deadline_and_closes_without_retry`,
was tested serially three times on each freshly compiled binary, without a
competing compiler or test process. Main passes attempts 1 and 2, then fails
attempt 3 at `src/receiver/tests.rs:1471` with the same `pair-verify did not complete`
and `/pair-verify M4 outer TLV exchange` / `response headers` / `receiver read timeout`
chain as the final PR run. Its mock receiver also reports `UnexpectedEof` at
`src/receiver/tests.rs:1250`. PR passes all three isolated attempts. The test
runner reports 287 filtered-out tests on main and 292 on PR, confirming the
expected revisions. Thus **all 11 final PR-failing tests have demonstrated
failures on current main under the same configured conditions**. This is evidence
of existing intermittent failures, not a claim that their underlying causes have
been fixed. No failing assertion or deadline has been changed.

Exact final failure excerpts (same trace/metadata omissions as above):

```text
receiver::tests::stalled_control_has_an_absolute_deadline_and_closes_without_retry
src/receiver/tests.rs:1471:47
called `Result::unwrap()` on an `Err` value: pair-verify did not complete
Caused by:
    0: /pair-verify M4 outer TLV exchange
    1: response headers
    2: receiver read timeout

receiver::tests::stream_tests::feedback_stream_identity_mismatch_fails_the_active_session
src/receiver/stream_tests.rs:808:5
assertion failed: session.failure().unwrap().contains("stream identifier")

receiver::tests::stream_tests::full_authenticated_realtime_session_encodes_encrypts_synchronizes_and_tears_down
src/receiver/stream_tests.rs:615:9
assertion failed: sync_count.load(Ordering::Relaxed) >= 3
src/receiver/stream_tests.rs:647:34
called `Result::unwrap()` on an `Err` value: audio scheduler stalled; no uncontrolled catch-up

receiver::tests::stream_tests::owntone_setup_same_socket_counters_validation_and_cleanup
src/receiver/tests.rs:50:35
called `Result::unwrap()` on an `Err` value: Os { code: 11, kind: WouldBlock, message: "Resource temporarily unavailable" }
src/receiver/stream_tests.rs:1563:10
called `Result::unwrap()` on an `Err` value: transient authentication M4
Caused by:
    0: /pair-setup M4 outer TLV exchange
    1: receiver operation timed out
    2: receiver read timeout

receiver::tests::stream_tests::record_tests::both_stream_modes_report_failed_remote_teardown_and_release_locally
src/receiver/record_tests.rs:642:74
called `Result::unwrap()` on an `Err` value: Os { code: 11, kind: WouldBlock, message: "Resource temporarily unavailable" }
src/receiver/record_tests.rs:849:9
assertion `left == right` failed: TeardownReject:
  left: 5
 right: 376

receiver::tests::stream_tests::record_tests::playback_check_receiver_decrypts_decodes_and_validates_bounded_pacing
src/receiver/record_tests.rs:820:9
assertion `left == right` failed: Good: Err(bounded TEARDOWN also failed: RTSP method=TEARDOWN: encrypted RTSP method=TEARDOWN phase=encrypted TEARDOWN elapsed_ms=262 response_bytes=0 authenticated_frames=0: response read/validation: response headers: receiver operation timed out: receiver read timeout
Caused by:
    audio scheduler stalled; no uncontrolled catch-up)
  left: false
 right: true

receiver::tests::stream_tests::record_tests::stua_persistent_failures_teardown_and_release_without_retry
src/receiver/record_tests.rs:849:9
assertion `left == right` failed: TeardownReject:
  left: 76
 right: 376

receiver::tests::stream_tests::record_tests::stua_persistent_playback_decrypts_and_decodes_fixed_faded_tone
src/receiver/record_tests.rs:820:9
assertion `left == right` failed: Good: Err(bounded TEARDOWN also failed: RTSP method=TEARDOWN: encrypted RTSP method=TEARDOWN phase=encrypted TEARDOWN elapsed_ms=263 response_bytes=0 authenticated_frames=0: response read/validation: response headers: receiver operation timed out: receiver read timeout
Caused by:
    audio scheduler stalled; no uncontrolled catch-up)
  left: false
 right: true

receiver::tests::stream_tests::record_tests::stua_selected_volume_reaches_encrypted_receiver_without_changing_tone
src/receiver/record_tests.rs:642:74
called `Result::unwrap()` on an `Err` value: Os { code: 11, kind: WouldBlock, message: "Resource temporarily unavailable" }
src/receiver/record_tests.rs:820:9
assertion `left == right` failed: Good: Err(bounded TEARDOWN also failed: RTSP method=TEARDOWN: encrypted RTSP method=TEARDOWN phase=encrypted TEARDOWN elapsed_ms=927 response_bytes=0 authenticated_frames=0: response read/validation: response headers: receiver operation timed out: receiver read timeout
Caused by:
    audio scheduler stalled; no uncontrolled catch-up)
  left: false
 right: true

receiver::tests::stream_tests::request_failures_attribute_each_negotiation_stage_and_close_channels
src/receiver/stream_tests.rs:24:36
called `Result::unwrap()` on an `Err` value: Error { kind: UnexpectedEof, message: "failed to fill whole buffer" }
src/receiver/stream_tests.rs:938:55
called `Result::unwrap()` on an `Err` value: pair-verify did not complete
Caused by:
    0: /pair-verify M4 outer TLV exchange
    1: response headers
    2: receiver read timeout

receiver::tests::stream_tests::transient_session_negotiates_encrypted_audio_and_acknowledges_teardown
src/receiver/stream_tests.rs:615:9
assertion failed: sync_count.load(Ordering::Relaxed) >= 3
src/receiver/stream_tests.rs:1008:30
called `Result::unwrap()` on an `Err` value: audio scheduler stalled; no uncontrolled catch-up

```

## Required check outcomes

| Command on updated PR | Outcome |
| --- | --- |
| `cargo fmt --check` | Pass, exit 0 |
| `cargo clippy --all-targets --locked -- -D warnings` | Pass, exit 0 |
| `RUST_TEST_THREADS=1 cargo test --locked` | Exit 101; 282 unit tests pass, 11 fail as recorded above |
| `cargo build --release --locked` | Pass, exit 0; optimized build completes in 41.50 s |
| `git diff --check` | Pass |

The first complete PR `--no-fail-fast` run and the valid complete main run both
pass all 29 integration tests. The final exact command skips those targets after
the unit target fails. No test is ignored, assertion weakened, dependency/lockfile
changed, or new receiver operation introduced. No hardware verification is run.
The suite still requires a stable-environment run before claiming an all-green
result; the preserved failures should be investigated separately from timing
address delegation. Source and asset contents match the tested merged tree.
