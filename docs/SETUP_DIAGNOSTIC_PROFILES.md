# Initial SETUP lifecycle experiments

The explicit OwnTone transient setup-only opt-in now starts its existing NTP
responder before initial SETUP, retaining it through TEARDOWN/cleanup. This is
separate from legacy A/B/C and playback; their lifecycle remains unchanged.
See [the pinned comparison](OWNTONE_SETUP_TIMING_COMPARISON.md).

PR #25 corrects the [initial NTP response contract](INITIAL_NTP_RESPONSE.md):
setup-only accepts an absent timingPort, while validating it strictly when present.

Based on fresh origin/main `01c06381606cbfc729f415cd7084af283df8ab87`,
which merged PR #23. The protocol audit and its independent crypto/plist
regressions remain applicable. These are explicit experiments, not an Apple TV
compatibility claim. Default playback and setup-only behavior remain profile A.

## Isolated comparisons

`stream-test --setup-check --setup-profile a|b|c` selects:

| Profile | Timing lifecycle | Request sequence on one TCP connection |
| --- | --- | --- |
| A (default) | Bind timing UDP socket before SETUP; no responder during initial SETUP; setup-only exits before the normal responder startup | Persistent verify M1/M3 (CSeq 1/2), encrypted GET /info (3), encrypted SETUP (4) |
| B | Bind once and start one ready NTP worker before SETUP | Same as A |
| C | Same as B | Plaintext GET /info (1), persistent verify M1/M3 (2/3), encrypted SETUP (4) |

All successful setup-only profiles validate the NTP response and ports, send
encrypted TEARDOWN (5), and close. None sends RECORD, stream SETUP, volume,
events, sync packets or audio. A/B keep existing transient authentication
available; C deliberately requires persistent credentials and never falls back.
Compare A vs B to isolate timing readiness. Compare B vs C to isolate request
ordering and which playback request consumes the first control nonce. Both
changes succeeding only together is also a possible result; three profiles
cannot independently determine the behavior of a fourth, late-timing/pre-info
combination. Repeat runs and alternate order to distinguish receiver state and
network variability from a reproducible profile effect.

A/B's one-frame GET uses TX nonce 0, SETUP uses TX nonce 1; C's SETUP uses TX
nonce 0. Receive counters advance per authenticated frame, independently of
CSeq. The same Wire, sender identity, parser, request envelope and keys persist
through verification; nothing resets counters or reconstructs the connection.
No experimental headers, PTP, FairPlay or audio changes are included.

## Pinned source comparison

Inspected OwnTone 29.3 at
[`d6fb3edf5831de38134ebd92fcf09a730ddd37aa`, airplay.c](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c):

- `airplay_init`, lines 4313–4315: `service_start` creates the shared timing
  service during output initialization, before a session's SETUP.
- `timing_svc_cb`, lines 2262–2338: receives a 32-byte `80 d2` probe with
  `recvfrom`, builds `80 d3`, copies the request transmit stamp to the reply's
  originate stamp, adds receive/transmit stamps and uses `sendto` to the source.
  The request source port is learned from the probe, not a SETUP reply.
- Sequence table at 3640 and `response_handler_info_start` /
  `response_handler_pair_verify2`: normal `/info` precedes verification; playback
  starts with session SETUP after cipher activation. See PR #23's source index
  for the surrounding cipher/sequence audit.

The existing PiAir `timing_reply` serializer is retained: it copies both request
sequence bytes (2–3), while OwnTone's callback copies byte 2 and leaves byte 3
zero. This PR does not introduce a timing-header experiment.

PiAir uses a per-session worker rather than OwnTone's global service. B/C move
the already-bound socket into that worker without cloning/rebinding it. A
bounded readiness channel acknowledges socket configuration, lease validation
and entry into the receive loop before SETUP can be written. No received probe
is required for readiness (that would deadlock a receiver waiting for SETUP).
Before remote timing ports are known, replies go only to valid probes from the
cryptographically verified TCP peer's IP, at the probe's source port. UDP is not
cryptographically authenticated; this restriction and the existing approval
lease checks limit the responder. Invalid and other-IP probes are ignored.

The narrow pre-verify method can only emit a bodyless GET /info on a fresh,
authorized connection. It retains strict RTSP 200/CSeq/length/duplicate-header
validation and bounded reads. Its plist is untrusted capability metadata: it
cannot change the selected endpoint, approve a receiver, install keys or bypass
the pinned Ed25519 verification. SETUP and timing still require secure control.
Any rejected info, verify or SETUP response fails closed, without retry.

The timing worker is owned by the negotiation's Ports value. Success, errors,
Ctrl+C, timeout and unwinding all drop it: stop flag, join, release socket. UDP
reads/writes have 100 ms limits; readiness has a 1 s limit. There is one worker
per live negotiation and no automatic retries. The existing optional 15 s
post-write SETUP deadline/cancellation polling is unchanged. Without it the
existing socket wait cap remains 3 s. Local cleanup does not wait a new full
request budget for TEARDOWN after failures.

## Tests and limits

The all-profile integration test performs actual persistent pair-verify, frame
encryption/decryption, CSeq checking and UDP exchanges. The mock withholds its
SETUP response until B/C answer a timing probe on the advertised port. A must
remain silent during that wait. It checks reply header, copied timestamp,
nonzero receive timestamp, same source/advertised port, and TEARDOWN without
RECORD/audio. Real crypto counters enforce the differing first encrypted
requests, including the subsequent encrypted TEARDOWN.

All-profile failure tests cover silent SETUP timeout, cancellation and a valid
encrypted reply with invalid plist semantics; each must release the timing port
and close control. A repeated worker test checks malformed/other-IP filtering,
valid replies, bounded drop, join via retained health ownership, and rebinding
across eight independent workers. A C-profile signature rejection test proves
successful plaintext info does not confer authentication. CLI tests enforce
setup-only scope, recognized profiles and authorization. PR #23's independent
asymmetric/multiframe crypto oracles and all earlier regressions are preserved.

Loopback mocks establish implementation invariants, not Apple TV behavior.
Readiness is a worker synchronization guarantee, not a real-time scheduling
guarantee. Forced kernel short writes, hardware packet loss, Raspberry Pi
scheduling and tvOS behavior are not simulated. No OwnTone/autostream service
was contacted, altered, stopped, restarted or reconfigured.

## Raspberry Pi commands after merge

Use the existing approved config, selected receiver and persistent pairing.
Replace only the config path and receiver ID below with their existing values;
do not copy/reset receiver-state or re-pair merely to run this experiment.
Run in an interactive terminal (stdin and stdout must remain terminals):

```bash
cd /path/to/PiAir
git fetch origin
git switch main
git pull --ff-only origin main
cargo build --release --locked
PIAIR_CONFIG_PATH=/absolute/path/to/existing/config.toml
PIAIR_RECEIVER_ID=device:aabbccddeeff
umask 077
mkdir -p setup-diagnostic-logs
git rev-parse HEAD > setup-diagnostic-logs/commit.txt
```

Create this allowlist filter to capture only fixed diagnostic fields, excluding
addresses, identities, URIs, raw headers, plist contents and arbitrary messages:

```bash
cat > setup-diagnostic-logs/sanitize.py <<'PY'
import json, sys
keys = {'profile', 'phase', 'method', 'cseq', 'status', 'elapsed_ms',
        'response_bytes', 'authenticated_frames', 'success', 'ready',
        'timing_port', 'timing_profile', 'body_length', 'exchanges'}
events = {'setup diagnostic selected',
          'NTP responder ready before initial SETUP',
          'control lifecycle: timing socket bound; responder deferred',
          'pre-verify info completed', 'authentication start',
          'initial SETUP start', 'initial SETUP RTSP response validated',
          'initial SETUP profile and ports validated',
          'authenticated request start', 'authenticated request completed',
          'encrypted control response status', 'authentication response',
          'receiver worker joined; socket released',
          'session cleanup: releasing sockets and joining workers',
          'session cleanup complete'}
for line in sys.stdin:
    try:
        record = json.loads(line)
    except (ValueError, TypeError):
        continue
    if record.get('target') not in {'piair::rtsp', 'piair::auth'}:
        continue
    fields = record.get('fields', {})
    safe = {key: value for key, value in fields.items() if key in keys}
    if fields.get('message') in events:
        safe['event'] = fields['message']
    if safe:
        print(json.dumps({'timestamp': record.get('timestamp'), **safe}), flush=True)
PY
```

Run each command separately and enter the exact receiver ID when prompted.
Only stderr goes through the sanitizer, preserving terminal approval checks:

```bash
./target/release/piair --config "$PIAIR_CONFIG_PATH" -v stream-test "$PIAIR_RECEIVER_ID" --setup-check --setup-profile a --setup-timeout-seconds 15 2> >(python3 setup-diagnostic-logs/sanitize.py > setup-diagnostic-logs/profile-a.jsonl)
./target/release/piair --config "$PIAIR_CONFIG_PATH" -v stream-test "$PIAIR_RECEIVER_ID" --setup-check --setup-profile b --setup-timeout-seconds 15 2> >(python3 setup-diagnostic-logs/sanitize.py > setup-diagnostic-logs/profile-b.jsonl)
./target/release/piair --config "$PIAIR_CONFIG_PATH" -v stream-test "$PIAIR_RECEIVER_ID" --setup-check --setup-profile c --setup-timeout-seconds 15 2> >(python3 setup-diagnostic-logs/sanitize.py > setup-diagnostic-logs/profile-c.jsonl)
```

Record each command's exit status (`echo $?` immediately afterward), receiver
model/OS, profile order, any Ctrl+C, and the terminal's success/failure outcome
separately. The filter intentionally omits non-JSON error text; do not interpret
its absence as success. Review the JSON before sharing. Do not capture/decrypt
or publish pairing material, keys or raw payloads. Compare status, elapsed time,
response bytes, readiness-before-SETUP and worker exchange/cleanup events.
Continued silence is a result, not evidence that PTP or any other speculative
change is required. Do not claim Apple TV compatibility until real hardware
results establish it.

## Local validation

Environment: x86_64 Linux, Rust 1.99.1. Rust was installed in a workspace-local
prefix; official Debian libasound2-dev 1.2.14-1+deb13u1 was extracted locally,
using the installed ALSA runtime. No toolchain or dependency lock changes are
included in this PR.

| Check | Result |
| --- | --- |
| `cargo fmt --check` | Pass |
| `cargo clippy --all-targets -- -D warnings` | Pass |
| `cargo test --locked` | Pass: 147 unit tests, 14 CLI tests (default parallel runner) |
| `RUST_TEST_THREADS=1 cargo test --locked` | Pass: 147 unit tests, 14 CLI tests |
| `cargo build --release --locked` | Pass |
| `git diff --check` | Pass |
| Documentation sanitizer, synthetic sensitive-field smoke check | Pass |

Socket tests ran with sandbox escalation to permit loopback. Initial runs
exposed test observation races (TCP EOF precedes client unwinding; UDP delivery
precedes counter update), now synchronized before asserting cleanup/counts.
The new failure test installs its own diagnostic subscriber so that first-use
tracing callsite registration does not disable an existing concurrent capture
assertion. All original assertions remain; production logging is unchanged by
this test fix. A subsequent overlapping
build/test run tripped the existing audio scheduler stall guard; the guard was
not weakened. The final default test run is performed without concurrent builds
or a second test process. These checks do not establish Raspberry Pi/tvOS
compatibility.
