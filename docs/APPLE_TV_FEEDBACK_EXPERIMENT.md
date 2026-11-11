# Apple TV WAV feedback experiment

This is an isolated hardware experiment following PR #55, **not a proven fix**
for silent Apple TV audio. The previous run authenticated, sent 1504 RTP packets,
received 8 NTP replies and completed TEARDOWN without audible audio. Those results
establish sender activity, not receiver media acceptance.

Authenticated WAV playback now uses the existing RTSP `POST /feedback` every two
seconds, including the drain period. Empty responses and the existing bounded
plist stream validation are preserved: any supplied streamID must match the
negotiated identifier and any supplied type must remain the negotiated realtime
profile (96). The short tone playback diagnostic keeps its existing policy.
Retransmissions remain disabled for WAV playback.

The same authenticated socket owns feedback and cleanup. WAV feedback has one
50 ms total write/read deadline, including partial encrypted frames; the normal
multi-second diagnostic response deadline cannot replace it. Cancellation and
authorization checks remain active. A successful poll must consume less than one
352-frame packet period (about 7.98 ms) and preserve the existing 100 ms scheduler
lateness bound. Otherwise playback fails before sending another RTP packet, with
bounded TEARDOWN where the control connection remains usable and unconditional
local cleanup. Timeout or transport errors invalidate the control connection;
there is no retry on that connection. This conservative experiment may stop on a
slow receiver or a busy Pi; it never silently continues after an excessive poll.

Debug diagnostics report `feedback_polling=true`, `retransmissions_allowed=false`,
numeric response status when available, fixed outcome classification and elapsed
time. Scheduler rejection has its own fixed classification. Receiver reason
phrases, headers, payloads, credentials and keys are not logged by these outcomes.

No RTP, ALAC, encryption, NTP, buffer, authentication, receiver selection or volume
behavior is changed. HomePod transient playback and persistent HAP authorization
use the same existing paths. No fallback or automatic receiver switching occurs.

On the isolated test Pi, with the operator present, an already approved and
explicitly selected Apple TV and existing persistent HAP pairing, run the reviewed
release build from the repository root:

```sh
./target/release/piair -v stream-test device:02d899c1833f \
  --audio-file tests/assets/audio/Pachelbel_Canon_audio_test.wav \
  --seconds 10 --volume-db -20
```

Retain interactive confirmation; volume may persist. Leave production OwnTone
and autostream unchanged. Expect 1504 RTP packets over about 12 seconds including
drain, feedback at roughly 2/4/6/8/10 seconds, and TEARDOWN/cleanup, unless feedback
or another existing safety check fails. Record audible music, onset, continuity
and playback indication separately from sender success. Also retain sanitized
feedback status/classification/timing and scheduler logs. No physical receiver
test has been performed as part of this change.
