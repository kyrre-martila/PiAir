# Initial NTP SETUP response validation (PR #25)

Base: fresh `origin/main` `785be7a22da116753f528d5e80e2b47be674873b`,
including merged PR #24. This corrects response validation, not diagnostic
profiles, defaults or playback transport behavior.

## Evidence and contract

User-supplied hardware result: AppleTV6,2, tvOS 26.6.1, profile B starts its
NTP worker before SETUP; persistent pair-verify and encrypted GET /info succeed.
Initial encrypted SETUP returns RTSP 200 in 168 ms, 312 response bytes, one
authenticated frame. The worker records three timing exchanges; encrypted
TEARDOWN returns 200. PiAir then reports `missing or invalid unsigned plist
field timingPort`. The full receiver plist has **not** been inspected: this
error alone does not establish its other fields or their validity.

Reinspected OwnTone **29.3**, pinned revision
[`d6fb3edf5831de38134ebd92fcf09a730ddd37aa`](https://github.com/owntone/owntone-server/tree/d6fb3edf5831de38134ebd92fcf09a730ddd37aa):

- [`airplay.c:3181–3250`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L3181),
  `response_handler_setup_session`: requires RTSP OK, parsed plist and nonzero
  eventPort. Does not read timingPort. Calls `handle_timingpeerinfo` only when
  `use_ptp` is true; this supplies no NTP endpoint.
- [`airplay.c:2262–2342`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L2262),
  `timing_svc_cb`: receives each timing probe with recvfrom and sends its reply
  to that probe's source address/port. There is no cached SETUP-derived receiver
  timing port in this lifecycle, nor a need to obtain one from stream SETUP.
- `payload_make_setup_session_ntp`, lines 2695–2707, advertises the **sender's**
  listening port; `airplay_init`, lines 4313–4315, starts that shared timing
  service before session negotiation. The advertised local port is never a
  substitute for a remote port.

PiAir now requires eventPort as an integer in 1..65535. Absent timingPort is
accepted for initial NTP validation; when present it must also be an integer in
1..65535. Missing timingProtocol remains accepted as before; when present it
must be the string `NTP`. Unsupported or incorrectly typed protocols fail.

## Code boundaries and remaining playback limitation

- `src/receiver/session.rs:330–358`: existing authenticated response, RTSP status,
  bounded binary plist and dictionary checks precede required eventPort and
  optional timingPort validation. `profile.rs:154–163` retains strict integer
  and nonzero u16 checks. No parser, Content-Type, CSeq, cryptographic counters,
  pinned identity, request URI or TEARDOWN identity checks are relaxed.
- `session.rs:353–358`: setup-only exits negotiation and calls `stop` for encrypted
  TEARDOWN (`check_initial_setup_with_timeout`, lines 269–272). Full playback
  instead explicitly requires timingPort before event connection or RECORD.
- `wire.rs:589–629`: unchanged full-playback `start_timing` connects UDP to the
  supplied remote port; `media` also requires it. No guessed or local port is
  passed here. Supporting a missing port in playback requires a separate
  migration to the probe-source responder lifecycle, including its filtering,
  readiness and health checks. This PR deliberately leaves that work open.
- `wire.rs:1112–1174`: B/C already reply to valid timing probes from the verified
  control peer IP at their source port. Their worker ownership, cancellation,
  joining and socket cleanup remain unchanged. UDP probes are not authenticated.

`stream_tests.rs:930–1057` covers both ports, absent timingPort, missing/zero/
out-of-range/negative/string/bool/real eventPort, the same invalid timingPort
values, unsupported/mistyped timingProtocol and explicit playback failure.
Each mock uses real persistent verification and encrypted SETUP/TEARDOWN on the
same URI, and expects EOF immediately after TEARDOWN, with zero audio packets.
`setup_profiles_order_crypto_and_readiness` covers A/B/C with no timingPort,
including B/C timing replies before SETUP response and socket release.
Existing full playback, independent crypto, cancellation and cleanup tests stay
in the full suite. No OwnTone implementation code was copied or translated.

## Validation

Checks run on x86_64 Linux with Rust 1.99.1 and workspace-local official Debian
ALSA development files. No dependency, lockfile or system service changes.

| Check | Result |
| --- | --- |
| `cargo fmt --check` | Pass |
| `cargo clippy --all-targets -- -D warnings` | Pass |
| `cargo test --locked` | Pass: 148 unit tests, 14 CLI tests |
| `cargo build --release --locked` | Pass |
| `git diff --check` | Pass |

The first test run found an incorrect new test expectation that cleanup would
change Failed to Disconnected. Corrected the assertion to verify the existing
preserved failure state; production cleanup was unchanged. Formatting, clippy
and the full default-parallel suite then passed. Socket tests ran with sandbox
escalation for loopback. These checks establish mock behavior, not hardware
playback.

## Raspberry Pi validation after merge

Keep the existing approval config and persistent pairing. Run interactively in
the PiAir checkout, replacing the two example values with existing values:

```bash
git fetch origin
git switch main
git pull --ff-only origin main
git rev-parse HEAD
cargo build --release --locked
PIAIR_CONFIG_PATH=/absolute/path/to/existing/config.toml
PIAIR_RECEIVER_ID=device:aabbccddeeff
./target/release/piair --config "$PIAIR_CONFIG_PATH" -v stream-test "$PIAIR_RECEIVER_ID" --setup-check --setup-profile b --setup-timeout-seconds 15
echo $?
```

Enter the exact receiver ID at the approval prompt. Expect exit 0 only after
valid initial SETUP and successful encrypted TEARDOWN, with no RECORD/audio.
Record commit, receiver model/OS, exit code, readiness, SETUP/TEARDOWN status,
response timing/frame counts and worker exchange/cleanup results. For retained
logs use the allowlist sanitizer in [SETUP_DIAGNOSTIC_PROFILES.md](SETUP_DIAGNOSTIC_PROFILES.md#raspberry-pi-commands-after-merge)
while keeping stdin/stdout attached to a terminal. Review logs before sharing;
do not publish raw payloads, identities, keys or credentials. Do not reset
pairing or modify/restart OwnTone or autostream. These commands have not been
run on hardware for this PR; no successful hardware playback is claimed.
