# PiAir architecture

PiAir is an original Apache-2.0 implementation targeting Raspberry Pi OS 64-bit
(Debian aarch64), with development on standard Linux. This milestone implements
configuration, passive DNS-SD discovery, capture-only audio activity tools,
and an explicit receiver pairing/authentication and encrypted control foundation. An explicit realtime UDP/ALAC/NTP sender is implemented;
hardware playback compatibility remains unverified. No code is copied, translated, or imported from Autostream, OwnTone/OwnTone
Mini, or airplay2-rs. Those projects may only inform publicly observable behavior
and protocol understanding. No existing AirPlay sender is a dependency.

## Current structure

- `main.rs`: one-shot command dispatch, local status and exit codes.
- `cli.rs`: clap commands and explicit configuration override.
- `config.rs`: TOML defaults, path resolution, validation, exact allowlist policy.
- `logging.rs`: JSON diagnostics on stderr, verbosity selected by CLI.
- `discovery.rs`: mdns-sd browsing, sanitized TXT, snapshot aggregation.

- `audio/mod.rs`: validated capture/detector settings and capture-only dispatch.
- `audio/capture.rs`: ALSA hints, signed S16/S32 interleaved negotiation,
  normalization, nonblocking reads, monitor and bounded same-device recovery.
- `audio/alac.rs`: original bounded stereo 48 kHz/S16 ALAC escape encoder and
  normalized sample conversion, validated using a test-only independent decoder.
- `audio/detector.rs`: pure stereo RMS windows, sample-clock FSM, intent events,
  validated timestamp continuity and bounded ordered pre-roll.
- `audio/calibration.rs`: bounded noise statistics/rejection, explicit prompts,
  comment-preserving TOML threshold edits and temporary-file persistence.

- `receiver/runtime.rs`: reusable automatic orchestration and typed invocation settings; shared by interactive automatic capture and explicit unattended `run`. See [RUNTIME.md](RUNTIME.md).
- `receiver/mod.rs`: exact identity operations, explicit method selection and
  approval/selection revalidation before enrollment persistence.
- `receiver/compatibility.rs`: pure requested-method decision over defensively
  parsed, untrusted capability evidence. Pairing evidence never proves playback.
- `receiver/wire.rs`: single current-approval TCP boundary, bounded RTSP/HTTP framing,
  encrypted continuation and per-operation approval/selection checks.
- `receiver/media.rs`: no-I/O receiver-observed encrypted packet components,
  sample timeline and bounded queue; no negotiated profile or receiver caller.
- `receiver/session.rs`: persistent authenticated control owner, typed lifecycle,
  typed negotiation, READY/STREAMING, bounded deterministic transmission,
  FLUSH/TEARDOWN, terminal failures and cleanup; buffered-audio gate remains closed.
- `receiver/profile.rs`: typed fixed-profile plist offers/responses and bounded
  binary-plist preflight; no speculative negotiation.
- `receiver/realtime.rs`: original realtime RTP/ALAC media encryption, cached
  retransmission, NTP replies/sync and absolute sample-clock math.
- `receiver/pairing.rs`: socket-owning persistent HAP setup/verify; displayed-PIN and
  existing-setup-code profiles, with no method fallback.
- `receiver/srp.rs`, `crypto.rs`, `tlv.rs`: HAP protocol composition and bounded messages.
- `receiver/store.rs`: separate version-2 secret storage, owned 0700/0600 permissions;
  actual signed receiver ID/key is distinct from optional advertised `pi`.
- `receiver/tests.rs`: loopback receiver, negative exchanges and authorization tests.

See [PAIRING.md](PAIRING.md) for protocol research, sequence diagrams, exact
supported profile, identity rules, cryptographic/storage limitations and recovery.
See [AIRPLAY_SESSION.md](AIRPLAY_SESSION.md) for the implemented encrypted control
profile and the precise media blockers. The explicit realtime sender reaches READY/STREAMING only after validated
negotiation. Buffered TCP remains disabled. Keep one `piair` executable and eventually
one systemd service. No web UI, database, Bluetooth, Spotify, media library, or
Docker requirement belongs in this architecture.

## Discovery and trust boundary

Browse `_airplay._tcp.local.` and `_raop._tcp.local.` using normal multicast DNS
queries, including DNS-SD SRV/TXT/address resolution. Never use RTSP, HTTP `/info`,
pairing, authentication, transport probing, or any direct receiver connection.
No service is registered. mdns-sd uses UDP 5353 (unprivileged) and local ephemeral
UDP signaling internally. No network session is initiated.

Each advertisement retains its own service type, instance, addresses, port, and
recognized capabilities. A snapshot updates by case-insensitive DNS instance name
and removes goodbye/expired entries. Canonical `device:<12 lowercase hex digits>`
IDs come from valid `deviceid` TXT or a RAOP `12hex@name` prefix. Matching IDs merge
services; conflicting valid IDs in one advertisement prevent merging. Missing or
malformed IDs fall back to `service:<lowercase full DNS instance>`. Fallback IDs
are stable across repeated snapshots of that instance, but may change on rename;
they cannot be allowlisted. Do not infer identity from a friendly name, shared IP,
hostname, or unsupported TXT field. Device IDs remain stable on name/IP changes
when the advertised ID is unchanged. DNS advertisements are unauthenticated and
can be spoofed; an allowlist is user intent, not cryptographic authentication.

The empty allowlist denies every future connection. Discovery lists unapproved
receivers explicitly and never selects a receiver. **Every session starts at `receiver::wire::Wire::connect`, which rechecks the current
allowlist and exact selected endpoint.** Negotiated socket factories remain in wire.rs,
use only authenticated peer/port bindings and recheck the same authorization lease. Check again after
configuration changes; authenticated credentials are bound to the authorized
stable receiver ID and the actual signed pairing ID. Advertised `pi` is only an
untrusted change-detection pin, never an authenticated identity assertion. Hiding devices in a CLI is insufficient. Unknown receivers
must never be contacted automatically. Pairing is now an explicit interactive flow; read-only discovery never invokes it.

## Compatibility and the next milestone

The [evidence ledger and compatibility matrix](PAIRING.md#compatibility-matrix)
separate specification facts, independent behavior reports and unresolved firmware
assumptions. Both persistent profiles share HKP-3 HTTP/1.1 setup/verify; a setup
code source is explicit and never discovered by trying methods. Transient,
MFi/legacy, Home-membership and password paths remain unsupported. Capability
bit 51 is a capability rather than a universal MFi requirement; an explicit
persistent-HAP selection with bit 46 may proceed, with the wire exchange checked
strictly for unsupported MFi requirements.

Merged PR #4's HAP/control and codec foundations are preserved. The continuation
uses a source-traced realtime AirPlay 2 NTP/UDP profile, not the earlier proposed
buffered profile. Source revisions, exact function traces and licenses are in
[AIRPLAY_REFERENCE_ANALYSIS.md](AIRPLAY_REFERENCE_ANALYSIS.md). Bit 51 no longer
causes blanket refusal when explicit persistent HAP bit 46 is present; unknown
MFi/transient exchange requirements still fail. Approval and signed identity
checks are unchanged.

All additional socket operations are confined to wire.rs and constrained to the
verified control peer plus authenticated validated ports. Event and timing workers
exist only for explicit diagnostic sessions, and are stopped/joined on cleanup.
No raw socket or secret API is available to the capture/activity engine.

## Future audio and network lifecycles

Current capture data flow (no network consumer). Capture remains disconnected
from the realtime sender; the older buffered
components remain disabled; [BUFFERED_RESEARCH.md](BUFFERED_RESEARCH.md)
records the historical buffered investigation. The current realtime profile is
described in AIRPLAY_SESSION.md. Capture/detector code never authorizes sockets,
SETUP or READY/STREAMING state transitions:

```text
ALSA capture -> PCM normalization -> RMS/activity detector -> intent events
                       |                                  ^
                       +---- bounded pre-roll ring --------+
```

Capture owns the ALSA handle, sample clock, sample validity, and bounded buffers.
It requests exact stereo, native S32 (preferred) or S16, and the nearest supported
rate. Actual rate/format/channels/period/buffer are read back. Unsupported formats,
non-stereo and rates outside 8..192 kHz fail. ALSA buffers over one second fail.
Two 2048-sample stack arrays accommodate short reads without per-read allocation.
The default ring is 768,000 bytes; maximum is 30,720,000 bytes, under 32 MiB.
A ring entry contains a u64 epoch frame index and two f32 normalized samples.
Oldest-frame eviction is expected and counted. Reset discards retained PCM and
partial windows. A future consumer must use frame indices to avoid duplicate
pre-roll/live handoff; startup latency/leading-edge preservation is unverified.

The detector uses f64 sum-of-squares per channel, maximum-channel dBFS, and exact
negative infinity for digital silence. Threshold comparisons are inclusive. All
qualification uses complete contiguous sample windows, rounding frame counts up.
Events Started/Stopped mean activity intent; Reset means discontinuity, not silence.
The capture supervisor reports faults and drops the entire epoch, then retries
at most three times with 250/500/750 ms interruptible backoff. Calibration rejects
any capture fault rather than recovering into a mixed measurement. No-data waits
poll every 10 ms and fail after two seconds; post-start scheduler gaps over one
second also fail. Ctrl+C sets an atomic flag; the ALSA handle closes on return.
Only the ctrlc crate's signal handler thread is needed; no capture worker/queue
or async framework is introduced. Monitor statistic scratch buffers are reused.

Calibration retains at most 60,002 window levels plus a sorting copy (under 1 MiB).
Median/p95 use nearest-rank percentiles. Reject fewer than 50 windows, nonfinite
or out-of-range levels, clipping, p95-p5 > 6 dB, max-median > 12 dB or p95 > -25
dBFS. Require at least 90% of the requested duration. Propose p95+6 stop and
p95+12 start. These conservative heuristics cannot prove a source is stopped;
explicit stopped-source confirmation and subsequent quiet-music tests are required.
Only confirmed threshold saves write configuration, preserving TOML decorations,
checking concurrent edits, syncing a temporary file and renaming it. New files
use create-if-absent linking. Concurrent replacement in the final check/rename
interval is not a filesystem compare-and-swap; do not edit the file while saving.

It can run while the network is idle, warming the detector and pre-roll. PCM
processing defines sample format/channel handling before activity is measured.
Use a bounded queue between capture and network; network backpressure must never
block capture or grow memory without limit. Overflow is observable and invalidates
continuity; it must not silently play stale music later.

The active session lifecycle owns explicit receiver approval, authentication,
timing, encoding, packet transport and teardown. Today it starts only through
interactive stream-test. Future activity-driven scheduling requires a separate
explicitly enabled capture integration; it is not implemented here. Capture errors
invalidate activity evidence. Network failure must not restart ALSA unnecessarily;
ALSA recovery must not implicitly reopen a receiver session. Reconnect remains
bounded, requires fresh activity and current approval, and drops obsolete audio.
The detector specification in AUDIO_ACTIVITY.md governs lifecycle transitions.

No installation, service management, privileged operation, or changes to existing
Autostream or OwnTone installations occur in this milestone. A future service
must have its own configuration, identity, and ALSA ownership policy; coexistence
must be measured rather than assumed.

## Dependencies and diagnostics

Use established crates for standard functions: clap (CLI), serde/toml (config),
tracing/tracing-subscriber (structured diagnostics), mdns-sd (DNS-SD), alsa
(ALSA bindings), ctrlc (signals), toml_edit (comment-preserving edits), libc
(errno constants only), anyhow
(errors). The application forbids unsafe code; dependencies may contain it.
Dependency metadata screening is recorded in DEPENDENCIES.md against the resolved
Cargo.lock; review redistribution notices before release. The original repository
LICENSE is preserved. Current
direct crates use permissive licenses (MIT/Apache-2.0; mdns-sd Apache-2.0 or MIT).
RustCrypto crates supply cryptographic primitives for the HAP profile; no AirPlay
sender dependency is used. SRP uses the crate arithmetic API with HAP-specific
hash/proof composition, checked against the published numerical vector. Additional
dependency screening is recorded in DEPENDENCIES.md. No dependency security audit
or complete process-memory wiping guarantee is claimed.

Logs must never dump raw TXT, configuration source, PCM, credentials, pairing
secrets, or key material. Only selected printable TXT capability fields are shown;
unknown fields, including public-key advertisements, are discarded. Protocol flags
are reported as advertisements, not verified playback support. Config parsing
errors omit source snippets. Discovery errors identify multicast/interface access
as a diagnostic path. CLI strings from advertisements are debug-escaped to keep
terminal control characters out of output.

mdns-sd's optional logging and async features are disabled: synchronous browsing
needs neither, and third-party trace messages must not expose raw TXT/key fields.

## Passive capabilities and transport planning

See [receiver planning and reference gap matrix](RECEIVER_PLANNING.md) for the
original typed evidence model, read-only group candidates, deterministic planner,
new `receiver-plan` command and the distinction between historical upstream
OwnTone research and the newer `owntone-mini/minimal` fork. Planning never opens
receiver connections or changes selection. Buffered/PTP remain unsupported for
streaming. Advertised buffered support and RTSP success do not prove audible playback.

Buffered type103 TCP ownership, bounded writes, fresh media-key lifetime and
original ALAC24 encoding now live in `receiver/buffered/`, independently of
realtime/NTP. See [the foundation and PR #41 boundaries](BUFFERED_TRANSPORT.md).
The session/CLI playback gate remains closed; no PTP clock is fabricated.


The [PR #41 PTP foundation](PTP_TIMING.md) lives in `receiver/ptp/`, independently
of realtime NTP: bounded PTP packets, a polled fixed sender grandmaster, precise
integer sample anchors, offset/drift analysis and typed timing control. Only
Wire creates production timing sockets after authentication/authorization; no
production caller exists before PR #42. The complete buffered lifecycle and
media gate remain disabled. Peer timing activity never asserts remote clock lock.
