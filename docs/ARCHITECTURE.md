# PiAir architecture

PiAir is an original Apache-2.0 implementation targeting Raspberry Pi OS 64-bit
(Debian aarch64), with development on standard Linux. This milestone implements
configuration, passive DNS-SD discovery, capture-only audio activity tools,
and an explicit receiver pairing/authentication and encrypted control foundation. AirPlay 2 playback does not work
yet. No code is copied, translated, or imported from Autostream, OwnTone/OwnTone
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

- `receiver/mod.rs`: exact identity operations, explicit method selection and
  approval/selection revalidation before enrollment persistence.
- `receiver/compatibility.rs`: pure requested-method decision over defensively
  parsed, untrusted capability evidence. Pairing evidence never proves playback.
- `receiver/wire.rs`: single current-approval TCP boundary, bounded RTSP/HTTP framing,
  encrypted continuation and per-operation approval/selection checks.
- `receiver/media.rs`: no-I/O receiver-observed encrypted packet components,
  sample timeline and bounded queue; no negotiated profile or receiver caller.
- `receiver/session.rs`: persistent authenticated control owner, typed lifecycle,
  terminal failures/idempotent cleanup and disabled buffered-audio gate.
- `receiver/pairing.rs`: socket-owning persistent HAP setup/verify; displayed-PIN and
  existing-setup-code profiles, with no method fallback.
- `receiver/srp.rs`, `crypto.rs`, `tlv.rs`: HAP protocol composition and bounded messages.
- `receiver/store.rs`: separate version-2 secret storage, owned 0700/0600 permissions;
  actual signed receiver ID/key is distinct from optional advertised `pi`.
- `receiver/tests.rs`: loopback receiver, negative exchanges and authorization tests.

See [PAIRING.md](PAIRING.md) for protocol research, sequence diagrams, exact
supported profile, identity rules, cryptographic/storage limitations and recovery.
See [AIRPLAY_SESSION.md](AIRPLAY_SESSION.md) for the implemented encrypted control
profile and the precise media blockers. No complete audio session implementation
is provided; READY/STREAMING remain unreachable. Keep one `piair` executable and eventually
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
receivers explicitly and never selects a receiver. **All connection code uses `receiver::wire::Wire::connect`, which rechecks the current
allowlist and exact selected endpoint at the connection boundary**, before opening any socket,
pairing, authentication, reconnect, or playback operation. Check again after
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
bit 51 is not misrepresented as a universal MFi requirement: the policy is blocked
because PiAir cannot safely determine/validate that enrollment policy.

PR #4 retains the identity/authorization boundary, capability policy, HAP
exchanges and schema-2 credentials. Successful pair-verify now transfers the live
wire and directional keys to a session owner; encrypted OPTIONS is available only
through the explicit authenticate diagnostic. Approval and selection are checked
at handoff and before/after control operations. Audio transport remains disabled:
SETUP/codec/media protection/clock/feedback evidence is incomplete. Hardware
behavior must be validated before claiming playback. No raw socket API or
automatic authentication fallback bridges these gaps.

## Future audio and network lifecycles

Current data flow (no network consumer). The standalone ALAC/media components
are compiled but not connected to this graph; [BUFFERED_RESEARCH.md](BUFFERED_RESEARCH.md)
explains the unresolved Apple TV profile. Components never authorize sockets,
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

The session lifecycle owns explicit receiver approval, authentication, timing,
encoding, packet transport, reconnect and actual playback teardown. It starts
only on confirmed activity and explicit receiver configuration. Capture errors
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
