# Initial SETUP timeout experiment (PR #22)

Current receiver discovery, selection and command usage is documented in
[Receiver diagnostics](RECEIVER_DIAGNOSTICS.md). Receiver-specific observations
and authorizations below are historical evidence, not current CLI restrictions.
Examples use placeholders; verify the canonical ID independently.

Start point: origin/main a3ebcb8, including merged PR #21.
Operator-confirmed evidence for AppleTV6,2 / tvOS 26.6.1:
pair-verify succeeds; encrypted GET /info returns 200 OK (1672 bytes,
2 authenticated frames); PTP is advertised. Initial encrypted SETUP is a
177-byte NTP plist at CSeq 4 and fails after 3177 ms with zero response bytes
and zero authenticated frames. PR #21's persistent sender identity did not
resolve it. These observations were supplied by the operator, not reproduced
on hardware here.

## Hypothesis and controlled change

Wire restarts a 15-second operation deadline per RTSP request, but caps socket
reads at 3 seconds and treats the first read timeout as fatal. This can explain
the observed duration; it does not explain why the receiver is silent.

Only `stream-test --setup-check --setup-timeout-seconds 15` opts into the
experiment. The option accepts exactly 15 and requires `--setup-check`.
After the initial SETUP request is written, one fixed 15-second response
deadline covers all encrypted frame lengths, ciphertext, authentication tags,
RTSP headers and body. Socket reads poll at most every 100 ms for cancellation;
poll timeouts continue the same read with its existing offset. No bytes, frame
or poll restart the response deadline. OS scheduling can slightly exceed it.
Ctrl+C aborts the response wait promptly and invalidates the connection.

Default reads still fail at their first socket timeout (at most 3 seconds,
or the remaining operation budget). Authentication, GET /info, request writes,
TEARDOWN and production streaming keep their existing behavior. The override
is cleared immediately after the initial SETUP call, on success or failure.
Payload, sender identity, NTP offer, headers and request order are unchanged.
Strict length limits, AEAD authentication/counters, CSeq, status, body, plist,
timing protocol and port validation remain in force. An invalidated control
connection skips TEARDOWN with a debug diagnostic instead of a secondary
cleanup failure; local resources are still released. Valid connections still
attempt TEARDOWN, and genuine cleanup failures are still reported.

## Raspberry Pi commands: existing pairing, no RECORD or audio

Use a separate checkout; keep existing services and executables unchanged:

```sh
git clone --branch diagnostic/setup-timeout --single-branch \
  https://github.com/kyrre-martila/PiAir.git PiAir-pr22
cd PiAir-pr22
git rev-parse HEAD  # compare with the PR head
cargo build --release --locked
install -D -m 755 target/release/piair "$HOME/.local/lib/piair-pr22/piair"

CONFIG=/absolute/path/to/existing/piair.toml
umask 077
"$HOME/.local/lib/piair-pr22/piair" --config "$CONFIG" -v \
  authenticate 'device:<canonical-id>' --control-check \
  2>piair-pr22-options.jsonl
printf 'OPTIONS exit status: %s\n' "$?"

# Optional default-timeout comparison, also without RECORD or audio:
"$HOME/.local/lib/piair-pr22/piair" --config "$CONFIG" -v \
  stream-test 'device:<canonical-id>' --setup-check \
  2>piair-pr22-setup-default.jsonl
printf 'Default SETUP exit status: %s\n' "$?"

"$HOME/.local/lib/piair-pr22/piair" --config "$CONFIG" -v \
  stream-test 'device:<canonical-id>' --setup-check --setup-timeout-seconds 15 \
  2>piair-pr22-setup-15s.jsonl
printf '15-second SETUP exit status: %s\n' "$?"
```

Run in an interactive terminal and type the exact receiver ID at the existing
confirmation prompt. CONFIG must point at the existing configuration and its
adjacent receiver-state directory. Do not pair again, unpair, copy pairing
secrets, change selection or relax approval checks. Both SETUP checks return
before RECORD, event/timing worker startup or any audio transmission.

## Reading the result

A successful SETUP response requires `initial SETUP profile and ports validated`:
200 alone is insufficient. Overall command success additionally prints
`Initial SETUP: validated NTP response and ports; TEARDOWN acknowledged; audio
packets transmitted: 0.` and exits zero. A valid SETUP followed by failed
TEARDOWN is a distinct cleanup failure, not a SETUP timeout.

A response timeout exits nonzero with `stage=initial SETUP`, encrypted
`method=SETUP`, elapsed time and progress counters. In the experiment, silence
should last approximately 15 seconds instead of 3. Zero response_bytes and zero
authenticated_frames mean no response bytes were read. Positive response_bytes
with zero authenticated_frames mean bytes arrived without a complete
authenticated frame. Positive authenticated_frames mean some frames passed
authentication; they do not prove a complete valid RTSP/plist response. Partial
responses use the same overall deadline. Peer closure, authentication failure,
invalid response and cancellation are separate failures, not successful SETUP.

Record the commit, model/OS, exit status, SETUP elapsed_ms, response_bytes,
authenticated_frames and validation/cleanup outcome. Share sanitized application
logs only, never receiver-state files or keys. A valid response after 3 seconds
would support the short-timeout hypothesis. Continued silence at 15 seconds
would show that a longer wait did not resolve this run; it would not prove PTP
is required or identify the receiver's reason for silence.

No physical Apple TV/HomePod verification, Raspberry Pi build or playback
success is claimed. PTP, codecs, streaming, FairPlay and protocol fallbacks are
outside this change.

## Local validation

Rust 1.99.1 on x86_64 Linux, using official Debian ALSA development files
extracted locally and the installed system ALSA runtime:

- `cargo fmt --check`: pass.
- `cargo clippy --all-targets --locked -- -D warnings`: pass.
- `cargo test --locked -- --test-threads=1`: 140 unit tests and 13 CLI tests pass.
- `cargo build --release --locked`: pass.
- `git diff --check`: pass.

The final suite ran with loopback sockets permitted for mock receivers; the
sandbox otherwise denies their binds. Tests cover CLI scope/defaults, silence,
partial encrypted frames and response deadlines, cancellation, a valid response
after three seconds, unchanged default waits, setup-only validation/cleanup,
and skipped TEARDOWN after transport invalidation. Existing authentication,
framing and independent wire fixtures pass unchanged. Hardware verification
remains pending.
