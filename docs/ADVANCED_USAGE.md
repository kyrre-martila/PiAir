# Advanced usage and protocol diagnostics

Moved from the README to retain detailed operator procedures and historical
evidence. Some sections describe earlier stages of development: their unverified
claims are scoped to those paths and stages. See [DEVELOPMENT.md](DEVELOPMENT.md)
for command classification and current validation limits. These commands remain
callable, including those hidden from top-level help.

## Buffered diagnostic status

Buffered transient diagnostics reuse the hardware-validated OwnTone-reference RTSP
authentication profile by default; see [evidence and remaining hardware checks](BUFFERED_LIFECYCLE.md#prompt-44-transient-authentication-compatibility-2026-11-10).

Buffered AirPlay 2 has an explicit, non-playing `buffered-test --setup-check`
diagnostic for one already selected and authorized receiver. Additional separately
confirmed `--control-check record|stream|anchor` stages exercise control negotiation
and a paused anchor without a media connection. See the [PR #43 evidence and
remaining roadmap](AIRPLAY2_COMPATIBILITY.md). Live audio stays
blocked pending receiver clock-adoption evidence. See the [lifecycle, limitations
and manual hardware instructions](BUFFERED_LIFECYCLE.md). No diagnostic runs
automatically; no buffered audible playback is claimed.

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

**Before adding, independently physically verify the ID** against the receiver's
label or trusted local device settings. An mDNS advertisement, advertised name,
or network address alone cannot establish physical identity and can be spoofed.
Adding asks you to attest to that verification, then confirm the exact change
with `y` or `yes`. Removing asks for change confirmation. Changes require stdin
and stdout to be interactive terminals; there is no unattended approval flag.
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
Pair/authenticate require an interactive terminal and explicit y/yes confirmation; pairing
reads the genuine PIN/setup code without echo. Connections recheck current
approval and pinned discovery evidence, with no retries, redirects or fallback.
No Samsung or other unapproved receiver is contacted. Read-only/config/audio
commands never open a receiver session.

Persistent HAP method 0 has two explicit profiles: `--method pin-display`
requires receiver-supported PIN display or equivalent independently verified PIN delivery; `--method setup-code` skips PIN-start and requires a genuine,
independently obtained `XXX-XX-XXX` HAP setup code. Both verify the complete SRP
and signed M5/M6 exchange before saving. The authenticated receiver identifier
comes from verified M6, independently of optional, untrusted advertised `pi`.
Pair-verify uses that stored identifier/key. By default `authenticate` then closes
its connection. Explicit `authenticate <id> --control-check` retains the same socket
for an encrypted RTSP OPTIONS exchange before closing. It requires the same
approval, selection, credentials and interactive y/yes confirmation. OPTIONS
success does not establish audio readiness or receiver playback compatibility.
See [session implementation, research and precise blockers](AIRPLAY_SESSION.md).

The active sender follows the [source-traced OwnTone NTP profile](AIRPLAY_REFERENCE_ANALYSIS.md):
AirPlay 2 persistent HAP, realtime UDP type 96, ALAC 44100/16/stereo, 352 frames,
encrypted events/media, NTP sync, feedback/retransmit and FLUSH/TEARDOWN.
That historical OwnTone inspection did not cover the pinned OwnTone-mini
buffered implementation. See the [current comparison](AIRPLAY2_COMPATIBILITY.md)
for type103 protocol evidence and PiAir's closed audio gate.

Normal realtime playback now uses the [early NTP responder lifecycle](REALTIME_NTP_LIFECYCLE.md)
verified for setup-only profile B. READY requires NTP synchronization and a healthy
[event connection](EVENT_LIFECYCLE.md); an idle event channel is valid.
Hardware playback remains unverified.

After reviewing the physical receiver and existing approval/selection/pairing,
the explicit diagnostic is:

```sh
piair --config config.toml stream-test device:aabbccddeeff --seconds 1
```

It requires an interactive terminal and explicit y/yes confirmation. It sends
1..3 seconds of deterministic stereo signal plus two seconds latency drain,
sets receiver volume to -30 dB by default (which may persist), and reports authentication,
negotiation, audio packets and acknowledged teardown separately. No audible
playback is claimed. PTP-only, other codecs and unsupported authorization paths
fail explicitly. No hardware testing was performed. See [session behavior,
bounds and limitations](AIRPLAY_SESSION.md).

For an initial SETUP check without RECORD or audio, use `stream-test --setup-check`.

For the explicit receiver-selected `--record-check --experimental-owntone-transient`
diagnostic, read the [pinned contract and manually authorized one-attempt
procedure](OWNTONE_RECORD_CHECK.md). It opens events, validates an empty
RECORD and tears down without stream SETUP, audio or volume changes. RECORD 200
does not prove stream negotiation or audible playback. No physical test is
authorized for Codex.

For an explicitly authorized compatible receiver, the transient opt-in is:

```sh
CONFIG=/absolute/path/to/existing/piair.toml
./target/release/piair --config "$CONFIG" -v stream-test 'device:<canonical-id>' --setup-check --experimental-owntone-transient
```

Use a separate checkout/binary and the existing configuration and receiver-state.
It requires this exact receiver to be selected as transient, terminal confirmation,
and unchanged allowlist/identity/endpoint checks. Do not contact Apple TV, change
production services, or replace deployed binaries. Rejects playback, persistent
selection and `--setup-profile` combinations, with no fallback. Read the
[exact Raspberry Pi build, authorization and single-attempt procedure](OWNTONE_TRANSIENT_SETUP_CHECK.md)
before running. Operator evidence dated 2026-11-08 confirms authentication and
encrypted OPTIONS on Even-Olai HomePod (192.168.1.33:7000, AudioAccessory5,1,
firmware 27.2). Task-supplied PR #34 evidence also confirms encrypted initial
NTP SETUP, active timing responder and acknowledged TEARDOWN. RECORD and audible
playback remain unverified.
Successful SETUP validates the response and ports, then requires acknowledged
TEARDOWN; it sends no RECORD, stream SETUP, audio or volume command. Transient
SRP does not establish persistent receiver identity.

The explicit `--setup-profile a|b|c` compares the existing lifecycle, early NTP
readiness, and early NTP plus OwnTone-style pre-verify `/info`. Defaults remain
unchanged. See [profile differences, tests and exact Raspberry Pi commands with
sanitized log capture](SETUP_DIAGNOSTIC_PROFILES.md).

A tested capability decision rejects unsupported combinations before connection,
including protected transient profiles, required MFi, password and Home-access paths.
Bit 51 with bit 46 can be explicitly selected for persistent HAP; it is a
capability, not proof that MFi is mandatory. Unexpected MFi exchange fields fail.
No method is inferred from model/name and no fallback occurs. Explicit transient
SRP authentication is implemented without persistent credentials or durable
receiver identity verification. See [transient flow, trust model and verification](TRANSIENT_AUTHENTICATION.md).
General Apple TV/HomePod playback compatibility remains unverified; the limited
HomePod authentication/control evidence above does not establish it. A usable
HomePod HAP setup code is not assumed. See the [compatibility matrix and evidence ledger](PAIRING.md#historical-baseline-compatibility-matrix-current-policy-corrections-included).
Mock success proves the implemented protocol path, not hardware playback.

Credentials live separately in `receiver-state/` next to config, with owned 0700
directory/0600 files on Linux. Filesystem permissions are the primary protection;
there is **no encryption at rest**. Corrupt/permissive state fails closed.
Version 2 separates authenticated identity from discovery metadata; older PR #3
credentials required explicit local unpair and re-enrollment at that historical
schema transition, and old selection files required reselection. This CLI cleanup
changes no schema and requires no new pairing or selection for current files. Partial receiver-side enrollment may require cleanup.
`unpair` removes local credentials without contacting the receiver; also remove
its allowlist entry to revoke contact approval. Remote pairing cleanup may require
receiver controls. See [protocol research, sequence diagrams, security boundaries,
recovery and compatibility gaps](PAIRING.md).

## Development

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
git diff --exit-code -- Cargo.lock
git diff --check
```

Tests use synthetic PCM, local files, published HAP vectors and loopback mock
receivers; no physical audio or real receiver is needed.
GitHub Actions runs these Rust checks automatically for pull requests targeting
`main` and pushes to `main`, and supports manual `workflow_dispatch` runs.
Use stable Rust with the rustfmt and Clippy components; on Debian/Ubuntu install
`libasound2-dev` and `pkg-config` first. All dependency-resolving checks use
`--locked` and verify that the committed `Cargo.lock` stays unchanged. New commits
cancel outdated runs for the same PR; other workflows are unaffected.

Release CI builds natively on GitHub's `ubuntu-24.04-arm` runner **inside pinned
Debian Bookworm**, checks ARM64 ELF/dependencies/glibc symbols, packages the WAV
and attribution, then checks startup in a clean Bookworm runtime. See
[INSTALL.md](INSTALL.md) and `.github/workflows/release.yml`. The former
Ubuntu-host build warning applied to earlier CI artifacts; current release
artifacts target Bookworm's glibc 2.36 baseline. For local source builds, use the
oldest supported Pi OS or an equivalent sysroot before distributing binaries.
This validates packaging/startup, not Raspberry Pi hardware playback. No Docker
is required on the Pi. Cargo.lock pins dependencies; builds are not claimed to be
bit-for-bit reproducible.

Tests need no operator input or privileged PTP port bindings: interactive tests
drive their own pipes/PTYs, ALSA tests use temporary file/null/plug fixtures, and
network tests use loopback mock receivers and unprivileged ports. Keep security,
authentication, playback and protocol regressions in the default test suite.
See
[architecture](ARCHITECTURE.md), the normative
[activity specification](AUDIO_ACTIVITY.md), and
[dependency screening](DEPENDENCIES.md). Physical S16/S32 USB capture,
hot unplug, overruns, long idle/quiet-music acceptance and shared-device behavior
require hardware validation. This is not a hard real-time scheduler; stdout and
OS scheduling can delay capture and produce reported discontinuities.

Original code is Apache-2.0; existing licenses are preserved. No source from
Autostream, OwnTone or airplay2-rs is imported or translated. No AirPlay sender
library, web UI, database, Docker dependency or automatic systemd installation is included.
Encrypted control and the explicit realtime audio sender are implemented and
mock-tested. Buffered live playback remains blocked. Legacy `audio monitor` activity events
never initiate network connections. The separate live and automatic playback
paths below integrate ALSA capture and detection under existing authorization.

For opt-in safe authentication logs and the HomePod 403 investigation, see
[AirPlay authentication diagnostics](AUTHENTICATION_DIAGNOSTICS.md).

Controlled hardware diagnostic: [explicit pyatv transient HTTP framing experiment](PYATV_TRANSIENT_PREAMBLE.md), disabled by default; HomePod support remains unverified.

See the [OwnTone-reference RTSP authentication-only experiment](OWNTONE_TRANSIENT.md) for the new explicit profile, verified protocol comparison and current hardware evidence.

For the pinned OwnTone control-channel audit, exact non-playback HomePod test
command, expected results and limitations, see [encrypted control verification](ENCRYPTED_CONTROL_CHECK.md).

### Interactive pairing and playback tests

```sh
piair pairing
piair receivers
piair stream-test
piair stream-test --seconds 3 --volume-db -15
piair stream-test device:xxxxxxxxxxxx --seconds 3 --volume-db -15
```

`xxxxxxxxxxxx` is a placeholder for your receiver's 12 lowercase hex digits.
The wizard shows one numbered line per receiver with a short status:
`[Not approved]`, `[Not paired]`, `[Selected]`, `[Paired]` or `[Paired, selected]`.
Not approved takes priority over local selection and pairing. Long names are
shortened; duplicate names include full stable IDs. Names are never identities.
Run `piair receivers` for full names, stable IDs, IPv4/IPv6 addresses, advertised
service types and ports, separate approval/selection/local pairing states and
compatibility diagnostics. This command is passive and does not change state.
Compare the full ID in the operation summary before confirming. Discovery is
untrusted and never proves receiver identity. Unknown
receivers are never automatically approved: independently verify the device,
edit `receiver_allowlist` in your configuration, then run `piair config check`
(with the same `--config` if used). The wizard deliberately does not edit config;
there is no general transactional allowlist persistence API.

Choose persistent credential enrollment only with its actual prerequisite:
PIN-display needs a receiver-supported display or equivalent independently verified
PIN delivery; setup-code needs a genuine, independently obtained HAP setup code.
An AirPlay password is not a HAP setup code. Do not invent codes or guess methods.
If neither is available, persistent enrollment cannot currently proceed through
these methods; cancel without changing selection or credentials. The wizard asks
for explicit prerequisite confirmation even for a saved persistent method.
For session-only diagnostics, follow the [current authentication evidence and safe
HomePod procedure](RECEIVER_AUTHENTICATION.md). It uses the existing explicit
OwnTone-reference command with separate authorization and no playback.
Existing selections retain their protocol profile and endpoint pins; unsupported
methods fail without fallback. Already-paired receivers return without connection
or credential replacement. Pairing saves cryptographically verified credentials;
authentication verifies a new connection. Local pairing is not current
authentication or playback readiness. Experimental authentication flags remain
available through explicit advanced `authenticate` commands, plus the narrowly scoped
OwnTone transient setup-check above.

Omitting the stream-test ID opens a numbered chooser; no first-device default.
A new selection requires an explicit method choice; receiver, method and endpoint
remain provisional until the final y/yes confirmation. Cancellation preserves
existing selection, credentials and allowlist. After confirmation and revalidation,
selection is saved immediately before receiver contact. A later operation failure
leaves that authorized selection in place; no stale selection is restored.
Concurrent selection changes cause an explicit failure instead of being overwritten.
Persistent methods require stored credentials before connection. Transient SRP is an explicit choice with
no persistent identity verification. Before connecting, existing allowlist,
endpoint, capability and credential checks still apply. Explicit receiver IDs
and advanced `select`, `pair`, `authenticate`, `unpair`, `--setup-check`,
`--setup-profile a|b|c` and setup timeout syntax remain supported.

All contact operations require a terminal and display the receiver name, full ID
and operation before confirmation. Only `y` or `yes` (case insensitive) authorizes.
Enter, n/no, other answers and EOF cancel with a failure status. Use `q` at a
numbered prompt or Ctrl+C to cancel; no pairing/playback success is reported.
There is no noninteractive confirmation bypass.

`--volume-db` takes an integer from -60 through -10 inclusive, default -30.
The selected volume is shown before playback authorization. **Receiver volume
may remain changed after testing.** This controls the existing receiver volume
parameter, not audio sample gain. Signal duration remains 1–3 seconds plus the
existing two-second drain. Setup-only checks never set volume or send audio.
Hardware and audible playback remain unverified; protocol/mock test success is
not evidence of sound from a physical receiver. No OwnTone/autostream changes
are required.

See [example terminal sessions and exact post-merge Raspberry Pi verification commands](CLI_VERIFICATION.md).

Diagnostics accept a canonical receiver ID or the existing interactive picker
when the ID is omitted. Use `piair receivers` to see friendly names and canonical
IDs; names can repeat, so verify the ID independently before allowlisting.

- `piair stream-test --record-check --experimental-owntone-transient`: choose an
  approved compatible receiver and explicit transient method; zero audio.
- `piair stream-test --stream-setup-check --experimental-owntone-transient`:
  validate realtime stream negotiation with zero audio or volume changes.
- `piair hap-stream-setup-check`: choose a receiver with existing stored HAP
  credentials and validate realtime stream negotiation without audio.
- `piair hap-playback-check 'device:<canonical-id>' --volume-db -20`: separately
  confirmed one-second tone and two-second silent drain. Volume (-40..=-10 dB,
  default -40) may persist after success or failure and is not restored.

The legacy `stua-stream-setup-check` and `stua-playback-check` names remain aliases.
All operations retain approval, capability, credential and endpoint checks and
terminal confirmation. Transient authentication does not verify persistent identity.
Buffered playback remains blocked by its existing media readiness gate. See
[receiver selection and diagnostics](RECEIVER_DIAGNOSTICS.md) for the selection
contract and [protocol details](OWNTONE_PLAYBACK_CHECK.md).
Automated checks do not establish physical routing or audible playback.

Passive transport diagnostics: `piair receiver-plan` shows observed capabilities,
provenance, group/stereo/proxy candidates and transport rejection reasons without
receiver connections or state writes. Optional `--method setup-code` (also
`pin-display`/`transient`) expresses planning intent only; it does not pair or play.
See [capability planning and remaining gaps](RECEIVER_PLANNING.md).

The [buffered AirPlay 2 foundation](BUFFERED_TRANSPORT.md) implements
type103 encrypted TCP framing and an independently decoded ALAC24 stereo codec.
The [PTP timing foundation](PTP_TIMING.md) adds a session-owned grandmaster,
precise 48 kHz anchors and authenticated timing-control requests. Complete buffered
playback remains disabled pending receiver clock-adoption evidence and media
lifecycle integration; staged control diagnostics are mock-tested only;
no buffered hardware playback is claimed.


For explicitly selected HomePod transient PTP WAV playback, use
`ptp-wav-playback --experimental-owntone-transient` after
`select device:0a05559f132f --method transient`. The default still requires stored
HAP credentials; no fallback occurs. See the [15-second Raspberry Pi test and
authentication limitations](PTP_IMPLEMENTATION_PLAN.md#explicit-homepod-transient-test-pr-64).

Linux/Raspberry Pi PTP playback binds privileged UDP 319/320. After each release
build or deployment, follow [PTP port permissions](PTP_PORT_PERMISSIONS.md)
to grant only `CAP_NET_BIND_SERVICE` to the executable and keep running as your
normal user.

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
