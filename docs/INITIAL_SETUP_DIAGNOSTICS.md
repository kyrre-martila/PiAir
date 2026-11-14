> For the current HomePod-only opt-in (PR #33), use the

Current receiver discovery, selection and command usage is documented in
[Receiver diagnostics](RECEIVER_DIAGNOSTICS.md). Receiver-specific observations
and authorizations below are historical evidence, not current CLI restrictions.
Examples use placeholders; verify the canonical ID independently.
> [OwnTone transient initial SETUP procedure](OWNTONE_TRANSIENT_SETUP_CHECK.md).
> The Apple TV experiment below is historical and is not authorized by that procedure.

# Apple TV initial encrypted SETUP investigation (PR #21)

Follow-up: [controlled 15-second SETUP timeout experiment (PR #22)](SETUP_TIMEOUT_EXPERIMENT.md).

Target: Example, 'device:<canonical-id>', 192.0.2.236:7000,
AppleTV6,2 / tvOS 26.6.1. User-supplied hardware evidence: persistent PIN
pairing, pair-verify and encrypted OPTIONS succeed; `stream-test --seconds 3`
times out during initial SETUP before audio. HomePod transient SRP and encrypted
OPTIONS also succeeded on hardware after #20. These results were reported by
the operator, not reproduced in this workspace.

## Findings and limits

The physical SETUP timeout's root cause remains unknown. A timeout is not proof
of plist rejection, PTP requirements, or a cryptographic failure. No physical
receiver or Pi pairing store is available here; the managed workspace has no
configured VPN. Hardware acceptance remains pending.

Proven from main at 4f329063734d16e189eff8f51485e2dfc4da7589 (merged #20):

* Pair-verify upgrades the same socket to encrypted control. Negotiation then
  sends encrypted `GET /info`, followed by the initial `SETUP`. No reconnect,
  counter reset or CSeq reset occurs. Each request increments checked CSeq;
  a pair-verify-only connection normally sends GET at 3 and SETUP at 4.
* The transport previously retained the authentication phase M3/M4 during these
  requests. Read failures therefore emitted misleading authentication labels.
  It now sets the encrypted RTSP operation before I/O and includes method,
  duration and progress in errors. Session errors still distinguish initial and
  stream SETUP. Numeric status is logged after header syntax validation; a 200
  is not successful negotiation until CSeq, body, plist and required ports pass.
* NTP SETUP already had the reference's four fields: deviceID string,
  sessionUUID string, timingProtocol string `NTP`, timingPort unsigned integer.
  Content-Type is application/x-apple-binary-plist; Content-Length is byte length.
  The shared authenticated envelope from #20 is retained. The URI is
  `rtsp://<local IPv4>/<decimal u32 session id>`, not the receiver endpoint.
* The old deviceID was made from five bytes of a new session UUID on every
  negotiation. It now uses PiAir's persisted u64 client identity, formatted as
  eight uppercase colon-separated octets, matching the reference's identifier
  model. All 64 bits are retained; no MAC truncation or borrowed receiver ID.
  Connect loads the same identity for Client-Instance/DACP-ID and Active-Remote
  before authentication. Existing OwnTone transient identity values are reused.
  A new installation creates its own random nonzero identity through the existing
  protected store. Do not copy receiver-state between independent senders.
  UUID v4 remains freshly generated per session; receiver-specific credentials
  and controller pairing identity are unchanged.
* Framing is unchanged: <=1024 plaintext bytes per frame, little-endian u16
  authenticated length, ChaCha20-Poly1305 tag, separate directional per-frame
  counters. Independent encrypted SETUP fixture remains unchanged. Tests cover
  multiple frames, fragmented responses, correlation, tags and strict cleanup.
* Requests restart the existing 15-second operation deadline. Each socket wait
  is capped at 3 seconds (or remaining deadline); a silent first read can fail
  after about 3 seconds. This PR does not lengthen deadlines or retry SETUP.
  On failure the connection is invalidated, keys cleared and socket shut down;
  cleanup does not get a fresh full timeout budget.

Reference: [OwnTone 29.3 airplay.c](https://github.com/owntone/owntone-server/blob/29.3/src/outputs/airplay.c),
functions `device_id_colon_make`, `payload_make_setup_session_ntp`,
`payload_make_setup_session_ptp`, `request_headers_add` and discovery's
`SupportsPTP` decision. Its device ID is a stable library hash, serialized from
u64 as eight octets. PiAir uses its own established identity, not that hash.
No OwnTone source was copied.

Feature bit 41 means SupportsPTP. OwnTone selects PTP only when advertised,
not disabled in device configuration, and its PTP daemon is available; otherwise
it permits NTP. The bit does not prove NTP is unsupported. PiAir logs authenticated
/info features and that bit, selects its existing NTP implementation, and does
not infer a PTP-only requirement from model/firmware. The existing Example regression advertisement in `src/receiver/tests.rs` uses
`0x80200,0x4000` (= 0x400000080200), which does not set bit 41. It is a
constructed test fixture, not a fresh tvOS 26.6.1 capture or authenticated /info
response; current Example timing capabilities still need hardware confirmation. There is no demonstrated NTP field/type mismatch;
plist dictionary ordering and integer storage width are not semantic differences.
PTP's different payload/clock behavior is not implemented. An explicit non-NTP
SETUP response still fails with an actionable unsupported-profile error, without
fallback, RECORD or media. OwnTone sends SETPEERS after RECORD, so its absence
cannot explain this initial SETUP timeout.

## Safe Raspberry Pi experiment using existing pairing

Run on the Pi in a separate checkout. Keep OwnTone mini, autostream and all
systemd units running and unchanged. Do not replace any service executable.

```sh
git clone --branch fix/initial-setup-diagnostics --single-branch \
  https://github.com/kyrre-martila/PiAir.git PiAir-pr21
cd PiAir-pr21
git rev-parse HEAD  # compare with the PR head
cargo build --release --locked
install -D -m 755 target/release/piair "$HOME/.local/lib/piair-pr21/piair"
```

Use the existing absolute configuration path in CONFIG. The receiver-state
folder next to that configuration contains the existing pairing and identity;
retain it in place. Do not pair again, unpair, copy secrets, change selection,
or approve another endpoint to make this experiment work. Discovery and approval
checks remain strict; stop and investigate if the selected receiver changed.

```sh
CONFIG=/absolute/path/to/existing/piair.toml
umask 077
"$HOME/.local/lib/piair-pr21/piair" --config "$CONFIG" -v \
  authenticate 'device:<canonical-id>' --control-check \
  2>piair-pr21-options.jsonl
"$HOME/.local/lib/piair-pr21/piair" --config "$CONFIG" -v \
  stream-test 'device:<canonical-id>' --setup-check \
  2>piair-pr21-setup.jsonl
```

Run in an interactive terminal and enter the exact receiver ID at the existing
confirmation prompt. `--setup-check` shares the real negotiation path through
initial SETUP, validates binary-plist content type, timing profile and nonzero
ports, then sends TEARDOWN and closes. It sends no RECORD or audio. Success must
include `initial SETUP profile and ports validated` and the CLI acknowledgement
of successful TEARDOWN. A 200 status alone is insufficient. Record the commit,
model/OS, endpoint, exit status and both outcomes. Playback remains unverified.

If SETUP still times out, retain the stderr JSON log from connect through
cleanup and the CLI error chain. Share those sanitized application logs, not
receiver-state files, pairing files, packet plaintext, credentials or keys.
Diagnostics include session URI, CSeq, header names, length, timing port, plist
field names/types, selected profile and /info features; they omit identity
values, arbitrary decrypted headers/bodies and pairing secrets.

Interpret `authenticated request completed` for encrypted SETUP:

* response_bytes=0: no TCP response bytes were observed before failure.
* response_bytes>0, authenticated_frames=0: bytes arrived, but no complete frame
  authenticated (partial length/ciphertext, framing or authentication failure).
* authenticated_frames>0: at least one frame authenticated; inspect numeric
  status and whether failure occurred in response headers, body or validation.
* `peer closed connection` differs from a read timeout. Neither establishes why
  the receiver closed or remained silent.

Counters describe bytes read from TCP (including framing/tag) and complete
frames authenticated for this request, not payload content or audio packets.
The next experiment changes only stable sender identity, keeping the NTP offer,
request agent, timing behavior and protocol order fixed. Compare with the
reported main failure before considering any further parameter change. If a
PTP-only requirement is demonstrated, use a sender with full PTP support; do not
retry arbitrary timing profiles or emulate a PTP clock.

## Local validation

Rust 1.99.1 on x86_64 Linux, with official Debian ALSA development headers
extracted locally and the installed system runtime:

* `cargo fmt --check`: pass.
* `cargo clippy --all-targets --locked -- -D warnings`: pass.
* `cargo test --locked -- --test-threads=1`: 136 unit tests and 13 CLI tests pass.
* `cargo build --release --locked`: pass.
* `git diff --check`: pass.

The initial suite found an outdated two-file store assertion; persistent sender
identity adds a third file. The expectation was updated while retaining checks
that credentials are unchanged and transient secrets are fresh. Full rerun
passed. Pairing cryptography and the independent SETUP fixtures are unchanged.
No Raspberry Pi cross-build, physical SETUP success or playback is claimed.
