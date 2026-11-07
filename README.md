# PiAir

A lightweight Rust AirPlay 2 transmitter project for Raspberry Pi OS 64-bit
(Debian ARM64). Implements ALSA capture, activity monitoring/calibration, passive
receiver discovery, persistent PIN pairing/authentication and encrypted control
session diagnostics and an explicit AirPlay 2 realtime UDP/ALAC/NTP audio test.
**Real Apple TV/HomePod playback compatibility remains unverified.** Audio tools never discover or contact receivers. Existing Autostream
and OwnTone services are never modified.

## Build on Raspberry Pi OS ARM64

Install a current stable Rust toolchain with rustfmt and Clippy, then build
natively as your normal user:

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libasound2-dev alsa-utils
cargo build --release --locked
./target/release/piair --help
./target/release/piair status
```

Check `uname -m` reports `aarch64`. For cross builds you also need an ARM64 C
linker, target ALSA development library and pkg-config sysroot matching the Pi's
glibc; installing only the Rust target is insufficient. Native ARM64 builds are
the recommended path. No Docker, systemd installation or root execution is needed.

## Identify and monitor a USB input

```sh
./target/release/piair audio devices
arecord -l
arecord -L
cp config.example.toml config.toml
```

Match the USB soundcard's ALSA card ID and capture device number. In `[audio]`,
set e.g. `device = "hw:CARD=USB,DEV=0"`, using your actual card ID. Named card IDs
are generally more stable than numeric card indices; identical USB cards may
still need a persistent ALSA/udev configuration. `audio devices` lists PCM hints
without opening a capture PCM; hints are not proof that a device is available.
`hw:` negotiates directly. `plughw:` or `default` can perform ALSA conversions;
the reported format is the application-facing negotiated format, not a claim
about the ADC's physical resolution or wiring.

```sh
./target/release/piair --config config.toml config check
./target/release/piair --config config.toml audio monitor
./target/release/piair --config config.toml audio monitor --duration-secs 15
```

Monitor shows the input, actual rate/format, two channels, ALSA period/buffer,
RMS window size, left/right and maximum-channel RMS in dBFS, thresholds, detector
state, qualification progress in frames, clipping counts, capture errors and
pre-roll health. Recent median/p95 summarize up to 250 windows (5 s by default);
these are recent audio statistics, not automatically learned noise thresholds.
Digital silence is `-inf`. Lower dBFS means quieter. An anti-phase or single
active channel still registers. Output updates about five times per second and
on activity events. `STREAMING` means confirmed activity intent; no AirPlay
session exists. Ctrl+C closes capture and exits cleanly. Finite durations are
1..86400 s and bound elapsed time, including recovery (ALSA open/configuration
calls themselves are driver dependent).

Only monitor/calibrate open the input. Status/config commands never do; ordinary
`devices` remains passive receiver discovery, distinct from `audio devices`.
Capture errors are visibly unavailable input, never silent audio. Faults discard
qualification, partial RMS windows and pre-roll. Monitor retries the same device
at most three times (250/500/750 ms backoff); persistent errors exit nonzero.
There is a two-second no-data timeout; a gap over one second after capture begins
also invalidates continuity. Restart monitor after fixing the cause.

## Calibrate the analog noise floor

```sh
./target/release/piair --config config.toml audio calibrate
./target/release/piair --config config.toml audio calibrate --duration-secs 10
```

Stop music and keep source volume, ADC gain, grounding and cabling representative.
Type `yes` at the source-stopped prompt. Calibration collects 30 seconds by default
(5..300 configurable), reports median and p95 of the louder channel's window RMS,
and proposes stop = p95 + 6 dB and start = p95 + 12 dB. It rejects faults,
clipping, nonfinite/digital-silence input, insufficient data, excessive spread or
transients, and apparently active input. Constant quiet music can resemble noise;
your stopped-source confirmation is essential.

Review the proposed and existing values and destination. A second explicit `yes`
is required to save only the two thresholds. Other TOML values and comments are
preserved. Concurrent edits detected before saving cause refusal. A same-directory
temporary file avoids truncating the configuration on write failure. No receiver
is contacted. EOF, `no` or interruption does not accept calibration.

**Test your quietest intended music after calibration.** It must exceed the start
threshold for the start qualification time. Quiet passages shorter than the stop
qualification time should remain STREAMING/STOPPING, and stopped-source noise
must remain below stop long enough to return to IDLE. Monitor several minutes of
idle input and varied music. If quiet music cannot start reliably while noise
stays below stop, improve signal-to-noise ratio/gain/cabling before lowering
thresholds. Never silently adapt thresholds during music. Recalibrate after
changing gain, cabling, ADC or source volume.

## Configuration and detector behavior

Resolution: `--config PATH`, `PIAIR_CONFIG`, then
`$XDG_CONFIG_HOME/piair/config.toml`, otherwise `$HOME/.config/piair/config.toml`.
XDG/HOME bases must be absolute. Only a missing default file uses built-in
defaults; a missing explicit file, malformed/unknown/wrongly typed setting or
invalid value fails with exit code 1. Help/version do not load configuration.
Read-only commands never create files. `config show` prints effective TOML;
`config check` validates it. Copy the example first for an explicit calibration
path. Calibration may create an absent default config only after confirmation.

| Audio setting | Default | Allowed range |
| --- | --- | --- |
| device | `default` | Nonempty ALSA name, up to 256 bytes, no control characters |
| sample_rate | 48000 Hz | 8000..192000 preferred; actual rate reported |
| channels | 2 | Stereo only; no downmix |
| window_ms | 20 ms | 5..100 |
| start_threshold_dbfs | -45 dBFS | Finite; stop < start <= 0 |
| stop_threshold_dbfs | -55 dBFS | Finite; -120 <= stop < start |
| start_duration_ms | 200 ms | 1..10000 |
| stop_duration_ms | 5000 ms | 1..120000 |
| pre_roll_ms | 1000 ms | 1..10000; at least start duration + one window |
| calibration_duration_secs | 30 s | 5..300 |

All thresholds are provisional until measured. RMS uses fixed complete windows
at the negotiated sample rate. Frame counts round up; qualification completes at
the first complete window covering the requested duration. Start requires
contiguous RMS >= start; stop requires contiguous RMS <= stop. In the hysteresis
band IDLE stays IDLE, STARTING cancels to IDLE, STREAMING stays STREAMING, and
STOPPING cancels to STREAMING. Short spikes cannot start; short pauses cannot
complete a stop. Faults reset to IDLE and require fresh qualification.

Pre-roll retains chronological normalized stereo PCM with sample-frame timestamps
within each capture epoch. It drops the oldest frame on overflow, reporting the
count. Default storage is 48,000 × 16 bytes = 768,000 bytes; the validated maximum
is 1,920,000 × 16 = 30,720,000 bytes (below 32 MiB). Calibration statistics use
less than 1 MiB combined storage/sort workspace; monitor statistics use 250 values.
No audio is played or sent. Future session startup may exceed retention; this PR
cannot establish leading-edge preservation on receivers.

## Coexistence and permissions

Run as a user with access to `/dev/snd` (desktop session ACLs or the appropriate
`audio` group on a headless Pi). Device busy/permission errors are actionable
capture failures. **Do not assume two processes can capture the same ALSA input.**
Use a separate input or an explicitly configured/tested ALSA sharing setup such
as `dsnoop` if another service owns it. PiAir never stops, reconfigures or installs
Autostream/OwnTone or any system service. Test ownership and sharing yourself;
using `default` does not guarantee simultaneous access.

## Receiver discovery and approval

`piair devices` and `piair receivers` perform bounded passive mDNS discovery only.
Stable IDs merge only by canonical device identity, never display name/IP/hostname.
Advertisements are untrusted; feature flags indicate advertised capabilities, not
verified compatibility. Unknown/conflicting IDs cannot be approved.

To approve a physically verified receiver, explicitly add its exact
`device:<12 lowercase hex digits>` to `receiver_allowlist` in your config.
The empty list denies every direct receiver connection. Then:

```sh
piair --config config.toml select device:aabbccddeeff --method pin-display
piair --config config.toml pair device:aabbccddeeff
piair --config config.toml authenticate device:aabbccddeeff
piair --config config.toml unpair device:aabbccddeeff
```

IDs above are examples. Selection pins identity/endpoints and never connects.
Pair/authenticate require an interactive terminal and typing the exact ID; pairing
requests a receiver PIN and reads it without echo. Connections recheck current
approval and pinned discovery evidence, with no retries, redirects or fallback.
No Samsung or other unapproved receiver is contacted. Read-only/config/audio
commands never open a receiver session.

Persistent HAP method 0 has two explicit profiles: `--method pin-display`
requests a receiver PIN; `--method setup-code` skips PIN-start and requires an
independently obtained `XXX-XX-XXX` HAP setup code. Both verify the complete SRP
and signed M5/M6 exchange before saving. The authenticated receiver identifier
comes from verified M6, independently of optional, untrusted advertised `pi`.
Pair-verify uses that stored identifier/key. By default `authenticate` then closes
its connection. Explicit `authenticate <id> --control-check` retains the same socket
for an encrypted RTSP OPTIONS exchange before closing. It requires the same
approval, selection, credentials and interactive exact-ID confirmation. OPTIONS
success does not establish audio readiness or receiver playback compatibility.
See [session implementation, research and precise blockers](docs/AIRPLAY_SESSION.md).

The active sender follows the [source-traced OwnTone NTP profile](docs/AIRPLAY_REFERENCE_ANALYSIS.md):
AirPlay 2 persistent HAP, realtime UDP type 96, ALAC 44100/16/stereo, 352 frames,
encrypted events/media, NTP sync, feedback/retransmit and FLUSH/TEARDOWN.
OwnTone's inspected sender does not implement buffered TCP type 103. PiAir's
older 48 kHz buffered packet components remain separate and disabled.

After reviewing the physical receiver and existing approval/selection/pairing,
the explicit diagnostic is:

```sh
piair --config config.toml stream-test device:aabbccddeeff --seconds 1
```

It requires an interactive terminal and exact receiver-ID confirmation. It sends
1..3 seconds of deterministic stereo signal plus two seconds latency drain,
sets receiver volume to -30 dB (which may persist), and reports authentication,
negotiation, audio packets and acknowledged teardown separately. No audible
playback is claimed. PTP-only, other codecs and unsupported authorization paths
fail explicitly. No hardware testing was performed. See [session behavior,
bounds and limitations](docs/AIRPLAY_SESSION.md).

A tested capability decision rejects unsupported combinations before connection,
including protected transient profiles, required MFi, password and Home-access paths.
Bit 51 with bit 46 can be explicitly selected for persistent HAP; it is a
capability, not proof that MFi is mandatory. Unexpected MFi exchange fields fail.
No method is inferred from model/name and no fallback occurs. Explicit transient
SRP authentication is implemented without persistent credentials or durable
receiver identity verification. See [transient flow, trust model and verification](docs/TRANSIENT_AUTHENTICATION.md).
No Apple TV/HomePod hardware/firmware is verified, and a usable HomePod HAP setup
code is not assumed. See the [compatibility matrix and evidence ledger](docs/PAIRING.md#compatibility-matrix).
Mock success proves the implemented protocol path, not hardware playback.

Credentials live separately in `receiver-state/` next to config, with owned 0700
directory/0600 files on Linux. Filesystem permissions are the primary protection;
there is **no encryption at rest**. Corrupt/permissive state fails closed.
Version 2 separates authenticated identity from discovery metadata; older PR #3
credentials require explicit local unpair and re-enrollment, and old selection
files require reselection. Partial receiver-side enrollment may require cleanup.
`unpair` removes local credentials without contacting the receiver; also remove
its allowlist entry to revoke contact approval. Remote pairing cleanup may require
receiver controls. See [protocol research, sequence diagrams, security boundaries,
recovery and compatibility gaps](docs/PAIRING.md).

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
git diff --check
```

Tests use synthetic PCM, local files, published HAP vectors and loopback mock
receivers; no physical audio or real receiver is needed.
GitHub Actions remains **manual-only** via `workflow_dispatch`. See
[architecture](docs/ARCHITECTURE.md), the normative
[activity specification](docs/AUDIO_ACTIVITY.md), and
[dependency screening](docs/DEPENDENCIES.md). ARM64, physical S16/S32 USB capture,
hot unplug, overruns, long idle/quiet-music acceptance and shared-device behavior
require hardware validation. This is not a hard real-time scheduler; stdout and
OS scheduling can delay capture and produce reported discontinuities.

Original code is Apache-2.0; existing licenses are preserved. No source from
Autostream, OwnTone or airplay2-rs is imported or translated. No AirPlay sender
library, web UI, database, Docker dependency or systemd installation is included.
Encrypted control and the explicit realtime audio sender are implemented and
mock-tested. Buffered TCP/PTP profiles remain unsupported. Current activity events
never initiate network connections; ALSA/silence/pre-roll integration is deferred.

For opt-in safe authentication logs and the HomePod 403 investigation, see
[AirPlay authentication diagnostics](docs/AUTHENTICATION_DIAGNOSTICS.md).

Controlled hardware diagnostic: [explicit pyatv transient preamble experiment](docs/PYATV_TRANSIENT_PREAMBLE.md), disabled by default; HomePod support remains unverified.
