# Realtime playback NTP lifecycle (PR #26)

The explicit OwnTone transient setup-only opt-in now starts its existing NTP
responder before initial SETUP, retaining it through TEARDOWN/cleanup. This is
separate from legacy A/B/C and playback; their lifecycle remains unchanged.
See [the pinned comparison](OWNTONE_SETUP_TIMING_COMPARISON.md).

Historical implementation record: PR #29 supersedes the event-traffic READY gate
and its earlier uncertainty below; see [EVENT_LIFECYCLE.md](EVENT_LIFECYCLE.md).

Base: fresh `origin/main` `67f6971`, including merged PR #25. No CLI,
dependency, codec, authentication or approval-policy changes.

## Evidence and source inspection

User-supplied physical hardware result, AppleTV6,2 / tvOS 26.6.1, PR #25
profile B: persistent cryptographic pair-verify and encrypted GET /info
succeed; NTP responder starts before initial SETUP; encrypted initial SETUP
returns 200 in 170 ms; initial NTP response and ports validate; three timing
exchanges complete; encrypted TEARDOWN returns 200; exit 0. No RECORD or audio
was sent. This proves that setup-only lifecycle on that receiver, **not actual
playback**. No hardware was contacted for this PR.

Reinspected the exact OwnTone 29.3 revision pinned by PR #25:
[`d6fb3edf5831de38134ebd92fcf09a730ddd37aa`](https://github.com/owntone/owntone-server/tree/d6fb3edf5831de38134ebd92fcf09a730ddd37aa).

| Reference in `src/outputs/airplay.c` | Protocol behavior |
| --- | --- |
| [`timing_svc_cb`, 2262–2342](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L2262) | Receives 32-byte timing probes, checks 0x80d2, replies with sendto to recvfrom's source address/port. No SETUP-derived timing endpoint. |
| [`payload_make_setup_session_ntp`, 2695–2707](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L2695) | Advertises the sender timing service port. |
| [`response_handler_setup_session`, 3181–3250](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L3181) | Requires eventPort, does not read an NTP timingPort; timing peer info belongs to PTP only. |
| [`airplay_init`, 4313–4315](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L4313) | Starts timing service before session negotiation. |

No OwnTone implementation code was copied or translated. PiAir retains its
independent worker, stricter packet validation and full two-byte sequence echo.

## Implementation and ownership

`Session::negotiate` binds local timing/control/data sockets after authenticated
GET /info. Playback always starts `Wire::start_timing` before initial SETUP;
setup-only A remains deferred, B/C remain early, with C's existing pre-verify
GET unchanged. The responder readiness handshake occurs after socket timeout
configuration and authorization validation. The actual bound local timingPort
is read before moving the timing socket into exactly one worker.

`start_timing` now reuses B/C's `Worker::timing_for_peer`. The obsolete connected
UDP worker and remote timing argument are removed. Only structurally valid
32-byte NTP queries from the verified control peer IP receive replies, at their
source UDP port. Invalid lengths/headers and other peer IPs are ignored without
terminating the worker. Any optional response timingPort still undergoes strict
unsigned 1..65535 validation; it is never used or synthesized as a destination.

`Session::ports` owns pending sockets/worker through event connection, RECORD
and stream SETUP. Health is checked before subsequent control requests.
`Wire::media` validates authentication, data/control ports and worker health,
connects only the authenticated stream ports, then transfers sockets/worker
once into MediaIo. A failed initialization retains pending ownership. Cleanup
sends best-effort encrypted TEARDOWN before dropping either pending Ports or
MediaIo. Worker Drop cancels, joins and releases the socket. Worker readiness
failure also drops and joins its partial worker. Cancellation, authorization
revocation, strict RTSP/plist validation and bounded cleanup remain in force.

Ready still requires a successful NTP exchange **and** an acknowledged encrypted
updateInfo event, with the existing three-second synchronization deadline,
worker health and five-second timing freshness checks. RECORD, ALAC tuple,
stream validation, latency limits, initial/periodic RTP synchronization,
retransmission, feedback, FLUSH and media encryption are unchanged. There is
no PTP, FairPlay, new codec, retry or fallback.

## Regression coverage and validation

The local mock receiver performs real persistent pair-verify and encrypted
control requests. The full playback test runs with and without receiver
timingPort, withholds initial SETUP's response until a correct NTP reply,
probes again during RECORD and stream SETUP from fresh UDP source ports, and
checks replies during TEARDOWN. Media initialization, independently decoded
ALAC, encrypted packets, synchronization and retransmission run in both cases.
Tests rebind the timing socket after cleanup. Existing tests cover malformed
NTP/wrong peer filtering, NTP/event Ready failures, worker panic reporting and
joining, authorization revocation, cancellation, failed negotiation, strict
port validation, A/B/C ordering, independent crypto and packet fixtures.
Normal-playback initial failure/timeout also verifies pending socket release;
semantic initial response failure verifies the responder during TEARDOWN.

Validation environment: x86_64 Linux, workspace-local Rust 1.99.1 and official
Debian libasound2-dev 1.2.14-1+deb13u1 extracted locally, using installed ALSA
runtime. No production services were modified, stopped or restarted.

| Check | Result |
| --- | --- |
| `cargo fmt --check` | Pass |
| `cargo clippy --all-targets -- -D warnings` | Pass |
| `cargo test --locked` | Pass: 149 unit tests, 14 CLI tests |
| `cargo build --release --locked` | Pass |
| `git diff --check` | Pass |

The added failure-path rebind assertion initially assumed TCP EOF implied all
local cleanup was complete. A control timeout can invalidate TCP before worker
joining finishes. Corrected that test synchronization: immediate rebind follows
successful shutdown only; failure/timeout tests wait for explicit local cleanup
completion. The final full suite passes with default parallel execution.

## Remaining protocol risks

Physical playback has never been established. Receiver acceptance of RECORD,
stream SETUP, encrypted events, media, latency and synchronization still needs
hardware testing. The required event updateInfo might not arrive on this tvOS;
no evidence supports weakening that gate. UDP timing is not cryptographically
authenticated: peer-IP filtering limits responses but does not prevent spoofing
or another process on the receiver IP. The per-session worker starts after
verification, unlike OwnTone's shared process-wide service; it is nevertheless
ready before negotiation as proven by profile B. Existing bounded IPv4/realtime
profile and short playback scope remain; no claim covers other devices or OSes.

## Safe Raspberry Pi test after merge

Run in a separate PiAir checkout with the existing config, selected receiver,
persistent credentials and interactive approval. Do not stop, restart, modify
or reconfigure OwnTone/autostream or any production service. If the receiver is
currently serving production playback, defer the test until it is available.
Do not reset pairing or bypass approval. Keep stdin/stdout attached to a terminal.

First update main, build and record the tested commit:

```bash
git fetch origin
git switch main
git pull --ff-only origin main
git rev-parse HEAD
cargo build --release --locked
PIAIR_CONFIG_PATH=/absolute/path/to/existing/config.toml
PIAIR_RECEIVER_ID=device:aabbccddeeff
```

Prepare the existing allowlist sanitizer exactly as documented in
[SETUP_DIAGNOSTIC_PROFILES.md](SETUP_DIAGNOSTIC_PROFILES.md#raspberry-pi-commands-after-merge).
It creates a private log directory and retains fixed diagnostic fields only,
excluding raw payloads, arbitrary errors, addresses, URIs and identities. Then
run **only profile B first**, entering the exact receiver ID at the prompt:

```bash
./target/release/piair --config "$PIAIR_CONFIG_PATH" -v stream-test "$PIAIR_RECEIVER_ID" --setup-check --setup-profile b --setup-timeout-seconds 15 2> >(python3 setup-diagnostic-logs/sanitize.py > setup-diagnostic-logs/profile-b.jsonl)
echo $?
```

Require exit 0, readiness before SETUP, valid encrypted initial SETUP and
TEARDOWN, joined worker and no RECORD/audio. Stop on any failure and review the
sanitized result; do not proceed automatically. After that succeeds, with the
receiver available and volume/output safe, manually approve one controlled
one-second playback attempt using the unchanged CLI:

```bash
./target/release/piair --config "$PIAIR_CONFIG_PATH" -v stream-test "$PIAIR_RECEIVER_ID" --seconds 1 2> >(python3 setup-diagnostic-logs/sanitize.py > setup-diagnostic-logs/playback.jsonl)
echo $?
```

This sets receiver volume to -30 dB and sends the existing bounded test signal
plus output-lead/drain frames. Ctrl+C invokes existing cancellation/cleanup.
Record model/OS, commit, exit code, NTP readiness/exchanges, RECORD/stream SETUP,
event synchronization, audio observation, packet counts and FLUSH/TEARDOWN
outcome. An RTSP 200 or exit 0 alone is not proof of audible correct playback.
Review logs before sharing. The filter omits non-JSON terminal errors, so record
a sanitized failure summary separately. Never share config/state files,
credentials, PINs, private keys, shared secrets, media keys, raw plist/headers,
packet captures or unfiltered logs. These commands have not run on hardware in
this PR.
