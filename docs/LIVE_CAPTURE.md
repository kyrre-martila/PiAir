# Live capture compatibility and validation

`experimental-ptp-capture` uses PiAir's existing authenticated PTP/type-96 owner.
No protocol, authentication, receiver approval or selection fallback is added.
WAV uses the same scheduler and its original timing/retransmission policy.

## OwnTone evidence

Reviewed the public OwnTone source at commit
[`8ed84f2b3cba25d3de65e05660d59ce034b5adab`](https://github.com/owntone/owntone-server/tree/8ed84f2b3cba25d3de65e05660d59ce034b5adab)
(2026-11-10). These are source observations, not hardware interoperability results.
No OwnTone source code was copied into PiAir.

| Area | Verified OwnTone behavior | PiAir decision |
| --- | --- | --- |
| Input architecture | [`src/inputs/pipe.c`](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/inputs/pipe.c): PCM pipe input, stereo, configurable 16/32-bit and 44100/48000/88200/96000 Hz. Empty reads/EAGAIN wait and retry for a manually selected pipe; fatal reads flag an error. This revision has no dedicated ALSA capture module. | Capture ALSA directly to keep a single lightweight CLI; convert with ALSA plug PCMs to the sender's required client format. Do not infer capture XRUN behavior from OwnTone's ALSA output. |
| ALSA conversion | [`docs/advanced/outputs-alsa.md`](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/docs/advanced/outputs-alsa.md) recommends `plughw` for transparent rate/format conversion when `hw` fails. This is **output** guidance. | Prefer `plughw`; retry a failed direct `hw:` negotiation through the same card/device's `plughw:` adapter. Other custom aliases can explicitly include a plug layer. Verify actual client PCM; do not mistake nominal hardware rate for the playback rate. |
| Input buffering | [`src/input.c`](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/input.c): a threshold equivalent to two seconds of 48 kHz/16-bit/stereo; `input_write` returns EAGAIN above it, and input waits use 10 ms. | A fixed 192 ms application queue drops oldest frames on overflow. This is a lower latency policy for a live source, not a configuration rejection or a claim that OwnTone drops samples. Total latency also includes ALSA and receiver buffers. |
| Scheduler/underrun | [`src/player.c`](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/player.c): 10 ms ticks, read/write catch-up thresholds of 1500 ms, then suspend/reset output. | Live allows 1500 ms bounded catch-up, below the existing two-second playback lead. Capture starvation fills RTP slots with silence; it does not reset timestamps or PTP. Longer sender stalls fail and teardown rather than sending unbounded catch-up bursts or replacing the session clock. WAV's 100 ms scheduler rule is unchanged. |
| ALSA XRUN/drift | [`src/outputs/alsa.c`](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/alsa.c): output EPIPE is classified as an underrun/session restart; `sync_correct` changes subscribed sample rate in steps to compensate ALSA **output** latency/drift. | There is no verified direct ALSA capture recovery/drift policy to copy. PiAir re-prepares XRUN/suspended capture and discards stale queued input, while the authenticated AirPlay session keeps its timeline. No adaptive resampler is justified by this output-only evidence. |
| AirPlay PCM | [`src/outputs/airplay.c`](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c): default AirPlay quality is 44100 Hz, 16-bit, two channels, with quality subscription/conversion and packet encoding in the output pipeline. | Retain the working negotiated ALAC16/stereo/44100 profile, packetization, encryption and synchronization. |

The reviewed evidence does **not** establish OwnTone's direct USB capture XRUN,
capture-to-AirPlay drift behavior, or Raspberry Pi 3B buffer requirements. PiAir's
96 ms prefill and 192 ms application queue remain experimental hardware-tuning
choices, not claims of measured equivalence with OwnTone. No blanket OwnTone
interoperability claim is made.

## Negotiation and recovery

The encoder requires signed native 16-bit interleaved stereo at nominal 44100 Hz.
ALSA's plug layer can expose that client interface while the physical device uses
a different sample rate, channel count or integer format. Exact client validation
prevents silently sending 48 kHz samples as 44.1 kHz or misinterpreting S32 data.
Period/buffer requests are hints: if tuning fails, negotiation retries without
them. Actual positive period and buffer sizes are accepted without packet-size
alignment or a one-second ceiling. All reads use fixed-size storage and honor
partial frame counts; a larger ALSA buffer does not allocate a larger PiAir queue.
ALSA's own buffering can still add latency; application queue bounds do not bound
unknown device/plugin or receiver buffering.

Nonblocking EAGAIN/EINTR waits retry; there is no runtime sample-gap deadline.
If a driver reports only EAGAIN after losing input, output remains silent until
recovery, a reported device error, network/timing failure, or Ctrl+C. Ten seconds
without initial prefill is treated as unavailable startup, to avoid leaving an
unusable authenticated session open indefinitely. PTP and feedback run throughout
startup; cancellation and authorization are checked at most every 5 ms between
bounded operations. ALSA waits are capped at 20 ms. A minimum wait and a bounded
read burst protect against broken/unclocked plug PCMs spinning without data.

For EPIPE/ESTRPIPE, capture re-prepares instead of calling `snd_pcm_recover`, whose
suspend resume loop may be unbounded. It discards the interrupted queue, increments
`recoveries` and counts discarded frames in `dropped_frames`. Silence occupies any
missing sender slots until new samples arrive. The PCM thread never holds the
queue lock during ALSA read/wait/recovery. Normal PCM transfers hold locks for at most one 352-frame batch; recovery clears
the fixed-capacity queue. Startup and control checks run outside that lock. Every terminal path stops and
joins capture before AirPlay TEARDOWN. Kernel/third-party ALSA plugins are expected
to honor nonblocking operations; this is not a guarantee against a hung driver
inside an open/prepare syscall.

## Long-running sessions

The live scheduler uses a renewing allowance of 1024 retransmission requests'
packet counts per two-second window, rather than a lifetime total. Requests remain
bounded to 1..32 packets each, work is bounded per iteration, cached packets have
three retries maximum, and the existing 256-packet cache stays bounded. Excessive
requests still fail closed. This avoids normal cumulative losses eventually
stopping a long record; it does not relax receiver authorization or nonce safety.
WAV retains its lifetime allowance.

The reviewed 30-second PTP setup deadline applies only until a peer is first
observed; it is not a playback lifetime. Existing three-second peer/worker
liveness checks and per-request feedback/cleanup deadlines are retained because
the sender must not continue against a stale clock, revoked receiver or broken
control connection. These are existing PTP security/clock-health boundaries,
not a new USB drift test. The existing 125 ms Sync/FollowUp association freshness
check also remains: it prevents announcing a stale PTP send time. PTP message
sequence numbers and RTP sequence/timestamps wrap normally; the independent
64-bit media nonce increases across wraps and exhaustion remains a terminal error.
The control connection's 32-bit CSeq counter also fails on exhaustion rather than
reusing a request identity (over 270 years at the normal two-second feedback rate).
No auth renewal/redial or automatic clock replacement is added.

## Automated validation and hardware limits

Hardware-free tests exercise:

- Actual ALSA plug conversion of file-backed 48 kHz S32 stereo and mono to the
  encoder's 44.1 kHz S16 stereo client interface (in isolated child processes).
- Partial reads, integer extremes, non-packet periods, large buffers, bounded
  overflow, silence insertion and explicit queue recovery.
- Twenty simulated minutes at each of ±100 and ±1000 ppm capture clock drift,
  using sample IDs to verify ordering and a bounded age/queue instead of only
  checking memory capacity. Six simulated hours of retransmission budgeting,
  including renewing allowance and per-window flood rejection.
- Scheduler catch-up policy and RTP continuity, plus the existing WAV tests.
- Mock capture startup errors, timeout, cancellation, device failure and cleanup.
  Authenticated loopback PTP tests assert source destruction before TEARDOWN,
  zero media on failed/cancelled startup, four-second delayed prefill with PTP
  still alive, network failure during startup/playback, duration expiry with silent drain, runtime device failure,
  Ctrl+C's cancellation flag, an unresponsive TEARDOWN peer and released sockets.

Run `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, and
`cargo test --locked`. Loopback socket tests need permission for local UDP/TCP;
no physical USB device, receiver, privileges for PTP 319/320, or root are needed.

Still required on Raspberry Pi 3B: identify the UCA202 by stable card ID; test
15 seconds and sustained vinyl playback; observe CPU use, XRUN counters and drift;
unplug/reconnect input; Ctrl+C during prefill and playback; and physically confirm
HomePod sound through the end of bounded playback. Synthetic tests do not establish
USB driver behavior, long-term audibility, actual system/receiver latency, or
OwnTone-equivalent hardware performance. See the README for exact commands.

## Automatic turntable mode

`automatic-ptp-capture` keeps the PR #66 ALSA worker open while monitoring,
connecting, playing and returning to monitoring. It runs in the foreground until
Ctrl+C or an unrecoverable capture error. No service, daemon or boot integration
is installed. Use the same approved, explicitly selected receiver and credentials
as manual playback; the terminal confirmation authorizes repeated sessions for
that receiver for this invocation. There is no receiver selection, occupancy
check, busy detection or fallback. Each new session reuses the existing identity,
authorization, authentication, PTP, RTP, feedback and bounded TEARDOWN paths.

For a Behringer UCA202 (commonly ALSA card ID `CODEC`; confirm with `arecord -l`):

```sh
piair automatic-ptp-capture '<your-approved-and-selected-receiver-id>' \
  --capture-device 'plughw:CARD=CODEC,DEV=0' \
  --threshold-dbfs -45 --start-duration-ms 1000 --stop-duration-ms 30000
```

Stored HAP authentication is the default. Only when you have explicitly selected
transient authentication, append `--experimental-owntone-transient`. There is no
automatic pairing or authentication fallback. Retain the existing PTP port
capability setup; running as root is unnecessary. Optional `--volume-db -20`
changes receiver volume and may persist afterward; omission preserves volume.

The detector measures RMS in nonoverlapping 20 ms windows at the live client
format of 44100 Hz S16 stereo. The louder channel determines activity; digital
zero is not required. Initial threshold **−45 dBFS RMS** (about 184 S16 counts)
is a starting point, not a calibrated UCA202 noise specification. Consecutive
above-threshold windows must cover **1 second** before connecting. Below-threshold
windows must cover **30 seconds** before stopping. An above-threshold window
resets the silence counter; sub-second clicks cannot qualify a start. Durations
round up to whole 20 ms windows. Detection is independent of RTP scheduling and
runs on incoming samples, including while authentication is in progress.

Arguments are independent of `[audio]` monitor/calibration configuration:

| Argument | Default | Accepted values |
| --- | --- | --- |
| `--capture-device` | required | Same explicit ALSA device as manual live capture |
| `--threshold-dbfs` | −45 | finite −120 through 0 dBFS |
| `--start-duration-ms` | 1000 | 20 through 9000 ms |
| `--stop-duration-ms` | 30000 | 20 through 120000 ms |

For vinyl, first measure idle input and a spinning silent groove with
`piair audio monitor`/`audio calibrate`, configured for the same input. Place the
threshold above the measured noise floor with several dB of margin, then check
quiet musical openings. Raise it (e.g. −40) if surface noise starts playback;
lower it (e.g. −50) if quiet records are missed. Increase the start delay for
handling noises and the stop timeout for long quiet passages. Surface noise
above threshold can keep a session playing after the record ends. A record with
very quiet openings may lose those openings before it qualifies as meaningful.
A threshold alone cannot distinguish music from sustained hum or interference.

### Buffering, stopping and recovery limits

Automatic mode retains at most **441000 stereo frames (ten seconds, 1.764 MB of
PCM payload)** in its shared queue; allocator capacity can be larger (under
4 MiB for this queue). Monitoring overwrites old samples. At qualified onset it
keeps the start duration plus 120 ms of recent history and retains newly captured
samples during discovery, authentication and timing readiness. The reader then
consumes the same ordered queue while the producer appends; no separate snapshot
is replayed and the capture thread never waits for network I/O. This intentionally
adds the retained backlog to end-to-end latency. The existing two-second AirPlay
playback lead is additional: buffered input receives fresh sequential RTP
sample timestamps at the current session's scheduling origin, rather than stale
capture wall-clock timestamps. There is no faster-than-realtime backlog flush.

If setup exceeds available space, oldest frames are discarded. The pre-roll
diagnostic reports retained frames and overflow since detection. Scheduling
stalls, XRUN recovery, capture starvation and clock drift retain PR #66's
explicit drop/silence policies. Complete preservation and gapless startup are
**not guaranteed**. The ten-second bound includes the activity qualification
period; a nine-second start delay leaves little connection headroom. After a
failure, a retry can only retain the most recent ten seconds of audio.

Once continuous silence qualifies, the sender latches EOF and transmits the
existing two-second silent drain on the same RTP/PTP timeline, then performs
TEARDOWN and releases network/playback workers. ALSA stays open; returning audio
can qualify a new session even during cleanup. Stops are driven by capture
silence, so a long backlog may discard its remaining tail at EOF; with the
30-second default that tail is silence. Very short configured stop timeouts may
cut delayed audible audio. Ctrl+C and protocol/security failures use the existing
bounded cancellation cleanup instead of promising a graceful media drain.

XRUN/suspend re-prepares the existing input, clears interrupted PCM and resets
activity qualification. Temporary EAGAIN/EINTR waits continue without busy
polling; missing samples do not count as measured silence. A driver that reports
only EAGAIN after input loss cannot qualify silence or a new start until samples
return; a previously playing session can remain open until another failure or
Ctrl+C. Explicit device/open errors are unrecoverable and release ALSA. Recoverable
AirPlay failures release the session and return to monitoring, with exponential
retry delays of 2, 4, 8, 16, 32 then at most 60 seconds. Retry also requires a
currently above-threshold RMS window (samples received within 100 ms) and an
already qualified activity interval;
silent input never causes a connection loop. Discovery and network operations
retain their existing bounded deadlines; Ctrl+C during them may wait for those
operations to observe cancellation or time out.

Diagnostics identify monitoring, qualified activity, connection, first media,
silence threshold, session stop, retry delay and ALSA recovery/errors. There is
no per-frame logging. Silence counters and qualification pause during missing
input; ALSA recovery explicitly resets them. Memory, queues and worker counts are
bounded; the monitoring loop waits 20 ms between checks.

### Validation required on equipment

The supplied project context reports that PR #66's manual live path played
continuously for 15 seconds from UCA202 to HomePod with zero drops, silence inserts
and recoveries. **Automatic mode has not been tested on actual equipment in this
PR.** Synthetic tests establish detector timing, queue ordering/bounds, continued
capture during connection, paced retries, EOF drain and authenticated cleanup;
they do not establish audible startup preservation or Pi 3B CPU use.

On Pi 3B/UCA202/HomePod, test quiet openings, surface noise, clicks, inter-song
gaps, thirty-second silence and a second record; delayed authentication and
buffer overflow; receiver loss and bad credentials; XRUN/input interruption;
and Ctrl+C while monitoring, connecting and playing. Measure audibility,
startup latency, CPU/memory, ALSA counters and released sockets after each cycle.
Manual `experimental-ptp-capture` and `ptp-wav-playback` retain their existing
options, defaults and timing policies.
