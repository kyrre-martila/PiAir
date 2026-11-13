# Event lifecycle and realtime READY (PR #29)

Base: fresh origin/main 920e710, including merged PR #28.

User-supplied Apple TV Example evidence (AppleTV6,2, tvOS 26.6.1): authentication,
initial SETUP, RECORD (Audio-Latency: 0), stream SETUP and SET_PARAMETER succeeded;
NTP was ready but no event arrived. The three-second synchronization wait failed,
TEARDOWN succeeded, and zero audio packets were sent. This identifies an unsupported
event-traffic gate; it does not establish that audio playback will work.

## OwnTone comparison

Inspected OwnTone 29.3 commit
[d6fb3edf5831de38134ebd92fcf09a730ddd37aa](https://github.com/owntone/owntone-server/tree/d6fb3edf5831de38134ebd92fcf09a730ddd37aa):

- `src/outputs/airplay.c`, response_handler_setup_session, lines 3216–3244:
  reads eventPort, connects the event listener and continues without an event.
  OwnTone even tolerates connection failure; PiAir continues to require success.
- `src/outputs/airplay_events.c`: client_add installs a HomeKit event cipher and
  read listener. Incoming ciphertext is decrypted before plist parsing.
  rtsp_parse recognizes sendMediaRemoteCommand string values play/paus/nitm/pitm;
  handle_event dispatches playback operations and respond sends encrypted RTSP 200.
  Unknown event types are ignored. Disconnect/decryption errors remove the client;
  deinit joins the thread and frees clients/ciphers/buffers.

No OwnTone implementation code was copied. PiAir retains stricter RTSP framing,
CSeq echo, Content-Length and Content-Type validation, 8192-byte header/body bounds,
1024-byte encrypted frames, authenticated control-peer identity and event keys.

## PiAir behavior

After authenticated initial SETUP, PiAir connects to the verified peer's eventPort,
installs separately derived event keys, validates authorization and configures the
worker before returning. An established idle socket is healthy: no heartbeat or
unsolicited updateInfo is required. READY still requires a successful NTP exchange
within three seconds, worker health and existing timing freshness checks.

Authenticated POST /command binary plists are classified as follows:

| Input | Outcome |
| --- | --- |
| updateInfo with dictionary value | Validated and acknowledged with encrypted RTSP 200; no state mutation |
| sendMediaRemoteCommand with string value | Acknowledged, explicitly ignored; no PiAir remote operations are implemented |
| Other nonempty string type | Ignored and acknowledged; never dispatched |
| Invalid known value, missing/invalid type, malformed plist/framing | Worker failure |
| Authentication tag failure or invalid encrypted frame | Worker failure; no acknowledgement or plaintext fallback |
| Unexpected EOF | Worker failure, including while otherwise idle |

Fixed debug messages distinguish established/idle, updateInfo, ignored command and
unsupported event without logging receiver-controlled values. Event counts are
observations only. Authorization is rechecked during idle polling and before replies.
Session health checks remain active during negotiation and transmission. Stop/failure
retains best-effort encrypted TEARDOWN, shuts down the event socket to interrupt reads,
then joins workers deterministically. Setup-only profiles A/B/C still return before
event connection and retain their existing timing/GET ordering.

## Verification and limitations

Loopback regressions exercise authenticated negotiation with an idle event socket,
READY, encrypted audio transmission and cleanup; NTP absence still fails. Event-worker
tests cover updateInfo, all four OwnTone command values, an unknown type carrying
play, malformed known values, bad tags, prohibited transfer framing and unexpected
EOF. Existing tests retain crypto/frame limits, partial-frame cleanup, negotiation,
revocation, cancellation and setup-only profile coverage.

Physical playback remains unverified. No hardware playback or production service
actions were performed for this PR. In particular, establishing an idle channel proves
TCP connection and authenticated key provenance, not possession of the event key by
the peer until authenticated traffic arrives. Remote playback control, PTP and
FairPlay remain unsupported; this change does not expand the media profile.

For physical verification, use the isolated checkout, existing credentials/approval,
and safe receiver availability procedure in
[REALTIME_NTP_LIFECYCLE.md](REALTIME_NTP_LIFECYCLE.md#safe-raspberry-pi-test-after-merge).
Do not stop/restart/modify OwnTone or autostream. Run setup-only B first, then manually
approve one bounded one-second stream-test. Record commit, model/OS, encrypted RTSP
statuses, NTP exchanges, READY, packet counts and FLUSH/TEARDOWN/worker cleanup.
An idle event channel should now permit READY after NTP; packets greater than zero
show only transmission attempted. Record any audible result separately; RTSP 200,
READY or exit 0 do not prove correct audible playback. Share sanitized diagnostics
only, never credentials or raw protocol payloads. Do not run these steps from Codex.

Validation environment: x86_64 Debian 13, workspace-local Rust 1.99.1, official
Debian ALSA development files extracted locally against the installed runtime.
No dependencies or lockfile changed. Loopback tests need execution outside the
agent socket sandbox; a sandboxed rerun failed with EPERM on socket creation,
then was rerun with approved local socket access.

Required checks: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test --locked`, `cargo build --release --locked`, and `git diff --check`.
All five checks passed: 153 unit tests and 14 CLI tests. Setup-only A/B/C ordering
and existing stream/cleanup regressions passed unchanged.
