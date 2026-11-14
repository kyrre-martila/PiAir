# Advanced usage

Normal use starts with `piair setup`; see [setup](SETUP.md) and [daily use](DAILY_USE.md).

## Build from source (developers)

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

A source/development executable can inspect the installed service's status and
logs, but cannot control that service. Only the trusted installed executable (or
a symlink to the same file identity) may do so. For isolated foreground tests,
use `piair run --help`. The simplified receiver picker lists
friendly names, rescan and cancellation; `piair receivers` and `piair devices`
provide technical details separately.

## Manage receiver approval locally

```sh
piair allowlist list
piair allowlist add device:aabbccddeeff
piair allowlist remove device:aabbccddeeff
piair --config ./config.toml allowlist list
```

Use the actual receiver ID: exactly `device:` followed by 12 lowercase hex
characters. Zero and broadcast IDs, names, IP addresses, service names, aliases,
and endpoint suffixes are rejected. Invalid or duplicate identities already in
the file must be corrected before these commands can proceed.

Adding asks once whether to approve the exact receiver ID for use with PiAir;
confirm with `y` or `yes`. You do not need to find this technical ID on a speaker
label or in its settings. Approval permits later connections; it does not
cryptographically verify receiver identity. Advertisements can be spoofed and
never trigger automatic approval. Removing also asks once for confirmation.
Changes require stdin and stdout to be interactive terminals; there is no
unattended approval flag.
Any other answer cancels without writing. Existing additions and absent removals
are reported as harmless no-ops without prompting.

Approval makes a receiver eligible for later operations; it does not prove
identity cryptographically or bypass selection, pairing, authentication or
playback gates. Removing approval does not delete stored credentials (use
`unpair` separately if needed). These commands never perform discovery, select a
receiver, connect, pair, authenticate, capture audio or play anything.

Resolution is unchanged: `--config`, then `PIAIR_CONFIG`, then
`$XDG_CONFIG_HOME/piair/config.toml` or `$HOME/.config/piair/config.toml`.
An explicitly named missing file is an error. A missing default file lists an
empty allowlist; a confirmed addition creates it and any missing parent
directories, with a private file mode. The resolved path is displayed.

Edits retain unrelated settings, TOML formatting, comments and existing receiver
entries, and validate the whole configuration before publishing. Existing file
permissions and ownership are preserved; read-only files and symlinks are refused.
Writes use a flushed temporary file in the same directory and atomic replacement,
followed by directory sync. Temporary files are cleaned up on pre-commit failures.
Allowlist, playback configuration and calibration writers take a nonblocking directory lock and reject stale content or
metadata after confirmation and immediately before replacement. If a competing
write is detected, rerun and review the new state. Other tools and manual editors
must honor the same directory lock to serialize the final replacement; avoid
manual editing of the same configuration concurrently. A directory-sync error
reports that the complete file was saved but durability could not be confirmed.

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

## Save automatic playback settings

After explicitly approving and persistently selecting your receiver, save its
operational settings once. Replace the placeholder ID with your approved HomePod:

```sh
piair config set-playback device:0123456789ab \
  --capture-device 'plughw:CARD=CODEC,DEV=0' \
  --authentication own-tone-transient \
  --threshold-dbfs -45 --start-duration-ms 1000 --stop-duration-ms 30000
piair run --allow-unattended
```

This example uses a Behringer UCA202 and a HomePod explicitly selected with
`--method transient`. Its values worked for one Raspberry Pi setup; they are
not universal hardware recommendations. `stored-hap` instead requires existing
HAP credentials and a persistent receiver selection. Neither profile is inferred.
Saving performs local validation only and never discovers, connects or captures.
It cannot approve, select, pair or move credentials.

`config set-playback` patches `[playback]`, retaining omitted values. For example,
`piair config set-playback --threshold-dbfs -48` updates only the effective
threshold. Optional `--volume-db -20` sets receiver volume on playback and may
persist remotely; `--preserve-volume` clears a saved volume. `run` accepts the
same flags as invocation-only overrides, plus an optional positional receiver ID.
Explicit CLI values override saved playback values, which override runtime numeric
defaults. Device, receiver ID and authentication have no runtime defaults and
produce actionable errors when absent. `[audio]` is never used as a fallback.

Existing files need no migration or repeated pairing. `[audio]` retains the
legacy two-threshold monitor/calibration detector; calibration changes only those
thresholds. Automatic playback retains its existing single-threshold detector.
There is no safe automatic conversion between those semantics. See
[the schema and runtime rules](RUNTIME.md#persistent-playback-configuration).
Operational changes apply after restart; security revocations remain live.
Saved settings never replace the mandatory per-invocation `--allow-unattended`.

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

### Experimental live USB input over PTP

This command uses the same authenticated type-96 ALAC16 stereo 44100 Hz sender
as `ptp-wav-playback`. It requires the existing explicit allowlist, selection,
authentication method and terminal confirmation. No receiver is selected automatically.
It does not require root; retain the existing CAP_NET_BIND_SERVICE setup for
PTP UDP 319/320. Explicit receiver volume changes may persist after shutdown.

Discover the capture card ID (do not copy a numeric card index):

```sh
arecord -l
arecord -L
piair audio devices
# Substitute the USB card ID printed above, e.g. CODEC, for <USB_ID>.
arecord -D 'plughw:CARD=<USB_ID>,DEV=0' -f S16_LE -r 44100 -c 2 -d 5 /tmp/usb-test.wav
```

With your already approved and selected receiver ID:

```sh
piair experimental-ptp-capture '<receiver-id>' --capture-device 'plughw:CARD=<USB_ID>,DEV=0' --seconds 15 --volume-db -20
piair experimental-ptp-capture '<receiver-id>' --capture-device 'plughw:CARD=<USB_ID>,DEV=0'
```

For an explicitly selected transient receiver, append
`--experimental-owntone-transient`, as with PTP WAV playback. Otherwise stored HAP
credentials are required. Omit `--seconds` to run until Ctrl+C. Omit `--volume-db`
to preserve receiver volume. Confirm the selected receiver at the terminal prompt.
Expected diagnostics include:

```text
Live capture: 44100 Hz S16 stereo; startup=96 ms; queue capacity=192 ms; underrun=silence; overflow=drop oldest
Live capture stopped: dropped_frames=0 silence_frames=0 recoveries=0; ALSA released
PTP operation: ...
```

The counters vary with scheduling and clock drift. Capture runs on a dedicated
nonblocking ALSA thread; the sender never waits for capture samples. Partial reads
preserve stereo order. ALSA's plug layer converts native USB PCM format and rate
to signed 16-bit stereo at exactly 44100 Hz. If direct `hw:` negotiation fails,
PiAir retries the same device through `plughw:`; custom PCM aliases can supply a
plug layer in ALSA configuration. Actual client rate, channels, format, period
and buffer are reported. Period/buffer tuning is optional; periods need not
match RTP packets and buffers larger than one second are accepted.
Startup queues twelve 352-frame packets (~96 ms); the queue
holds at most twenty-four (~192 ms). Overflow drops oldest frames, keeping recent
audio instead of accumulating latency. Empty queue slots become silence; RTP
sample timestamps and periodic PTP synchronization continue on the same clock.
XRUN/suspend recovery re-prepares ALSA, discards the old capture queue and does
not restart that clock. Temporary missing input becomes silence without an
arbitrary runtime gap timeout; explicit device errors terminate playback and
invoke the existing bounded teardown. Startup waits up to ten seconds for prefill,
while continuing PTP, authorization, feedback and cancellation checks. Capture
readiness waits are bounded to 20 ms, and workers join before TEARDOWN.
There is no silence detection. Clock drift can occasionally cause drops or silence.

Indefinite live sessions have no media-duration limit. Their retransmission
allowance renews every two seconds; bounded work, three retries per cached packet,
nonce exhaustion checks and receiver authorization remain enforced. Live sender
scheduling tolerates up to 1.5 seconds of catch-up (below the existing two-second
playback lead); WAV retains its original 100 ms policy. PTP liveness expiry,
network failures and unrecoverable capture errors still stop a live session.
See [live capture compatibility and validation](LIVE_CAPTURE.md) for the
OwnTone source comparison, technical reasons for retained limits, and tests.

If opening fails, check `arecord -l`, capture permissions and other programs using
the card. Use `plughw:` for sample-rate/format mismatch. For missing audio, first
verify the short arecord recording and the UCA202 input connections; enable the
turntable's built-in phono preamp and confirm receiver volume. Successful packet
transmission alone does not prove sound. AirPlay receiver buffering adds latency
beyond the capture queue. Rising `silence_frames` indicates capture starvation;
rising `dropped_frames` indicates scheduling delays or capture/playback clock drift.
Check CPU load, USB power and network quality on the Pi before changing buffers.
Physical UCA202/Pi 3B capture, unplug recovery, sustained clock drift, Ctrl+C and
HomePod audibility require hardware validation and cannot be established in CI.

### Automatic turntable playback

After approving and selecting your receiver, monitor a Behringer UCA202 and
start/stop AirPlay sessions automatically (confirm the ALSA card ID first):

```sh
piair automatic-ptp-capture '<receiver-id>' --capture-device 'plughw:CARD=CODEC,DEV=0'
```

Defaults: −45 dBFS RMS threshold, one second of sustained audio to start, thirty
seconds of continuous silence to stop. Tune with `--threshold-dbfs`,
`--start-duration-ms` and `--stop-duration-ms`. ALSA stays open between sessions;
Ctrl+C exits. Existing receiver approval, explicit selection, stored credentials
and terminal confirmation remain required. Explicit transient authentication
still requires `--experimental-owntone-transient`. The ten-second pre-roll is
bounded and cannot guarantee preserved openings after slow setup or overflow.
See [automatic mode tuning, buffering and hardware validation](LIVE_CAPTURE.md#automatic-turntable-mode).

For foreground automatic playback without prompts, see [runtime and unattended operation](RUNTIME.md), including explicit opt-in, authorization, shutdown and exit codes. An optional [systemd service template](SYSTEMD.md) supervises this same runtime using saved configuration, non-root execution and systemd-managed PTP capabilities. Installation and boot enablement remain explicit; see [INSTALL.md](INSTALL.md) for the release installer.
