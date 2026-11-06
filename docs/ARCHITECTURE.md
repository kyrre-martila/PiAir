# PiAir architecture

PiAir is an original Apache-2.0 implementation targeting Raspberry Pi OS 64-bit
(Debian aarch64), with development on standard Linux. This milestone implements
configuration and passive DNS-SD discovery only. AirPlay 2 playback does not work
yet. No code is copied, translated, or imported from Autostream, OwnTone/OwnTone
Mini, or airplay2-rs. Those projects may only inform publicly observable behavior
and protocol understanding. No existing AirPlay sender is a dependency.

## Current structure

- `main.rs`: one-shot command dispatch, local status and exit codes.
- `cli.rs`: clap commands and explicit configuration override.
- `config.rs`: TOML defaults, path resolution, validation, exact allowlist policy.
- `logging.rs`: JSON diagnostics on stderr, verbosity selected by CLI.
- `discovery.rs`: mdns-sd browsing, sanitized TXT, snapshot aggregation.

No placeholder audio or session implementations are provided. Future modules
`audio/capture`, `audio/pcm`, `audio/activity`, and `session` should be introduced
when they have real behavior and tests. Keep one `piair` executable and eventually
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
receivers explicitly and never selects a receiver. **Future connection code must
enforce the allowlist at the connection boundary**, before opening any socket,
pairing, authentication, reconnect, or playback operation. Check again after
configuration changes and bind authenticated identities to user approval once
authentication exists. Hiding devices in a CLI is insufficient. Unknown receivers
must never be contacted automatically. Pairing will require a later explicit flow.

## Future audio and network lifecycles

Planned data flow:

```text
ALSA capture -> PCM normalization -> RMS/activity detector -> session/streaming
                       |                                  ^
                       +---- bounded pre-roll ring --------+
```

Capture owns the ALSA handle, sample clock, sample validity, and bounded buffers.
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
tracing/tracing-subscriber (structured diagnostics), mdns-sd (DNS-SD), anyhow
(errors). The application forbids unsafe code; dependencies may contain it.
Dependency licenses and transitive notices must be reviewed against the resolved
Cargo.lock before release; the original repository LICENSE is preserved. Current
direct crates use permissive licenses (MIT/Apache-2.0; mdns-sd Apache-2.0 or MIT).
No cryptographic or AirPlay sender dependency is needed yet. Future crypto must
use reviewed standard primitives, not handwritten cryptography.

Logs must never dump raw TXT, configuration source, PCM, credentials, pairing
secrets, or key material. Only selected printable TXT capability fields are shown;
unknown fields, including public-key advertisements, are discarded. Protocol flags
are reported as advertisements, not verified playback support. Config parsing
errors omit source snippets. Discovery errors identify multicast/interface access
as a diagnostic path. CLI strings from advertisements are debug-escaped to keep
terminal control characters out of output.
