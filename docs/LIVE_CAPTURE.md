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
