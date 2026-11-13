> Scope: this specification describes the legacy `[audio]` monitor/calibration
> detector in `audio/detector.rs`. Automatic AirPlay uses the existing independent
> single-threshold detector in `audio/activity.rs` and `[playback]` settings for
> `run`; see [RUNTIME.md](RUNTIME.md#persistent-playback-configuration). Calibration
> only changes `[audio]` thresholds and is not converted into automatic settings.
> Historical future-session requirements below are not a description of that runtime.

# Audio activity detector: binding specification

This document is normative for detector and future session work. Capture,
monitoring, calibration and activity intent are implemented. Actual session
requests/teardown below remain requirements for a later milestone; this PR emits
Started/Stopped/Reset intent events only and has no receiver connections.

## Measurement and parameters

Measure RMS over fixed, timestamped PCM windows (initial design: 20 ms). Normalize
integer PCM by its full-scale amplitude into [-1, 1]; reject nonfinite floating
samples and invalid/discontinuous input. For each channel compute
`rms = sqrt(sum(sample^2) / N)` and `dBFS = 20 * log10(rms)`. Exact digital silence
is negative infinity dBFS. Use the maximum channel RMS for activity so anti-phase
stereo or one active channel does not disappear through downmix cancellation.
Do not use peak amplitude as the activity measure. State sample format, sample
rate, channel count, window size, RMS dBFS, and clipping counts in monitoring.

Configuration must expose separate start/stop thresholds and durations. Require
`stop_threshold_dbfs < start_threshold_dbfs <= 0`, finite thresholds, positive
durations, and validated buffer bounds. Provisional examples: start -45 dBFS,
stop -55 dBFS, start duration 200 ms, stop duration 5 s, pre-roll 1 s. **Every
threshold value is provisional until measured on real hardware**, including ADC,
gain, grounding, cabling, and source volume. These settings now live in the validated `[audio]` TOML table.

## Finite-state machine

Use captured sample time for contiguous qualifying durations, never wall-clock
time across dropped data. Define start qualification as RMS >= start threshold;
stop qualification as RMS <= stop threshold. Values between the thresholds form
the hysteresis band. State changes must be observable and deterministic.

| State | Condition | Required transition/action |
| --- | --- | --- |
| IDLE | First qualifying start window | STARTING; begin contiguous start timer |
| STARTING | Every window continues qualifying for start duration | STREAMING intent; request approved session and ordered pre-roll |
| STARTING | Any window below start threshold | IDLE; reset start timer |
| STREAMING | First qualifying stop window | STOPPING; begin contiguous stop timer |
| STOPPING | Every window continues qualifying for stop duration | Terminate actual playback/session, then IDLE |
| STOPPING | Any window above stop threshold | STREAMING; reset stop timer without reopening session |

STREAMING is detector intent, not proof that a network session is playing. Expose
detector state and actual session state separately. Network setup failure must not
be reported as playback. In the hysteresis band IDLE stays IDLE, STREAMING stays
STREAMING, STARTING cancels, and STOPPING cancels. At boot digital silence stays
IDLE; short pauses under the stop duration preserve the existing session; isolated
noise spikes under the start duration cannot open a session. A new start after a
completed stop must pass the full start qualification again.

A quiet but nonzero analog noise floor **must not keep streaming indefinitely**.
Calibrate thresholds above measured idle RMS with headroom and validate long idle
recordings. If hardware noise exceeds configured thresholds, monitoring must make
the cause visible and guide recalibration. Do not silently adapt thresholds during
music. Do not treat every nonzero PCM sample as activity.

On completion of STOPPING, terminate the **actual AirPlay playback/session** with
protocol teardown and resource release. Indefinitely transmitting silence is
forbidden. Teardown is bounded; if acknowledgment fails, close transports and mark
the session closed locally with an error. Silence packets during a short confirmed
pause may be protocol-required, but are not a substitute for stopping playback.

## Calibration and monitor CLI

`piair audio monitor` is capture-only and must never open a receiver
connection. Show current per-channel/combined RMS, noise-floor
summary, thresholds, state, timers, clipping, buffer health, and input errors.
Tests must cover meter accuracy with known synthetic signals and silence.

Calibration requires the user to confirm the source is idle and hardware gain is
representative. Collect a bounded sample (provisional: 30 seconds), summarize
median and high-percentile window RMS, reject input errors/clipping/unstable or
apparently active input, and propose thresholds with explicit margins. Show the
proposed settings, units, and evidence. Require user confirmation before saving;
never overwrite thresholds silently. Confirm intended quiet music can trigger
start with a subsequent real recording. Calibration and monitor are read-only
with respect to receivers. Their capture-device permissions must be documented.

## Pre-roll, faults, and reconnect

Maintain a bounded timestamped PCM ring while capture is healthy, including IDLE
and STARTING. Retain enough pre-roll to include the leading edge plus detector
qualification and measured session startup latency; validate the configured memory
budget. Start playback with chronological pre-roll and then live PCM without
duplicate samples. Set a bounded setup budget. If setup outlasts retention, report
lost leading audio explicitly; never claim clipping was avoided. Measure this on
real receivers, as startup latency may exceed the provisional one-second buffer.

Input errors, overruns, invalid samples, or timestamp gaps reset qualification,
invalidate pre-roll continuity, and trigger bounded teardown of any session.
They are not synthesized silence and cannot satisfy start/stop timers. Recover
capture with bounded backoff; resume in IDLE with a fresh ring and full start
qualification. Permanent faults remain visible and must not loop rapidly.

Network loss closes stale session state while capture may continue independently.
Reconnect needs a currently allowlisted, explicitly selected receiver and fresh
confirmed activity; never scan for or substitute another receiver. Backoff and
attempt count must be bounded. If activity stops during backoff/setup, cancel the
attempt and discard obsolete queued audio. Do not replay an old pre-roll on every
retry. Recheck approval at every connection boundary and cancel on revocation.

## Required acceptance tests

Deterministic synthetic PCM tests must cover zero -> -infinity, known sine RMS,
one-channel and anti-phase stereo, threshold equalities, hysteresis, start/stop
duration boundaries, short pauses, single spikes, sustained analog-like noise,
clipping, NaN/input gaps, and restart from every state. Fake session tests must
assert teardown rather than endless silence, no connection during IDLE/monitor/
calibration, approval on reconnect, bounded queues and retries, and cancellation
on inactivity. Real hardware acceptance must measure idle noise, quiet music,
leading-edge preservation, startup latency, and actual receiver playback ending
within stop duration plus a documented bounded teardown budget.

## Current implementation bounds

See README for exact setting ranges and commands. At 48 kHz, the default one-second
ring stores 48,000 timestamped stereo f32 frames (768,000 bytes); maximum validated
retention is ten seconds at 192 kHz (30,720,000 bytes, below 32 MiB). Pre-roll must
cover start qualification plus one window; actual receiver startup latency is
unknown. Frame timestamps restart only in a new capture epoch after reset. Events
and borrowed chronological pre-roll expose future controller inputs without any
network setup. A future controller must consume Reset as a teardown request and
track frame indices for a duplicate-free pre-roll/live handoff.

Clipping counts samples at or within one S16 least significant bit of full scale;
this conservative rule also covers f32 rounding of S32 full-scale values. Capture
normalizes S16 by 32768 and S32 by 2147483648. Window frames and qualification
frames round up at the actual negotiated rate; transitions occur only at complete
window boundaries. Calibration rejects faults rather than bridging capture epochs.
No session latency, network teardown or receiver playback tests apply until a
session implementation exists. Hardware noise, USB recovery, shared-device access
and quiet-music sensitivity remain target-hardware acceptance work.

For automatic playback, use `piair setup` and its actual input-level measurement
step ([SETUP.md](SETUP.md)). It uses `audio/activity.rs` and `[playback]`; the legacy
two-threshold monitor/calibration described here does not configure automatic mode.
