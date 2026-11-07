# Audio activity detector: binding future implementation specification

This document is normative for future detector and session work. Nothing here
claims current audio capture, monitoring, calibration, or playback support.

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
gain, grounding, cabling, and source volume. These settings are not in the current
config schema because no detector is implemented yet.

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

A future `piair audio monitor` (name provisional) must be capture-only and never
open a receiver connection. Show current per-channel/combined RMS, noise-floor
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
