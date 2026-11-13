# Apple TV WAV feedback experiment

This is an isolated hardware experiment following PR #55, **not a proven fix**
for silent Apple TV audio. The previous run authenticated, sent 1504 RTP packets,
received 8 NTP replies and completed TEARDOWN without audible audio. Those results
establish sender activity, not receiver media acceptance.

Authenticated WAV playback now uses the existing RTSP `POST /feedback` every two
seconds, including the drain period. Empty responses and the existing bounded
plist stream validation are preserved: any supplied streamID must match the
negotiated identifier and any supplied type must remain the negotiated realtime
profile (96). The existing general realtime polling path uses the same worker;
the short tone playback diagnostic keeps polling disabled.
Retransmissions remain disabled for WAV playback.

During WAV playback one feedback worker exclusively owns the existing authenticated
control connection, including its CSeq, pending input and directional cipher
counters. The RTP scheduler submits a poll every two seconds through a bounded
channel and checks outcomes without waiting. At most one request is outstanding;
there is no overlapping request, socket clone, reconnect or shared-socket mutex.
Normal encrypted TCP round trips of 15–40 ms do not delay RTP. The former
one-packet-period response rejection has been removed. The existing RTP scheduler
lateness guard remains unchanged.

Each feedback request has one 500 ms total write/read deadline, including partial
encrypted frames. The normal diagnostic response deadline cannot replace it.
Reads check cancellation and authorization between waits capped at 100 ms.
Rejected, invalid or failed feedback is returned to the playback thread and fails
the session; it is never treated as best effort. After RTP stops, the worker is
joined and the same connection is returned before TEARDOWN. Pending outcomes are
checked even when they arrive after the last RTP packet. Cancellation and error
paths also join the worker before bounded remote cleanup and unconditional local
release. Transport failure invalidates control; it is never reused or retried.

Debug diagnostics report `feedback_polling=true`, `retransmissions_allowed=false`,
numeric response status when available, fixed outcome classification and elapsed
time. Receiver reason phrases, headers, payloads, credentials and keys are not
logged by these outcomes.

Tests withhold feedback replies for 15 and 40 ms while independently receiving
RTP packets, and verify full packet count, sample timeline, successful playback
and ordered TEARDOWN. Rejection, invalid stream type, timeout, peer closure and
cancellation exercise failure cleanup. Additional channel tests check that an
outstanding poll cannot block progress or queue a second request, late errors
propagate on join, idle workers are joined, and worker panic becomes an error.

No RTP, ALAC, encryption, NTP, buffer, authentication, receiver selection or volume
behavior is changed. HomePod transient playback and persistent HAP authorization
use the same existing paths. No fallback or automatic receiver switching occurs.

On the isolated test Pi, with the operator present, an already approved and
explicitly selected Apple TV and existing persistent HAP pairing, run the reviewed
release build from the repository root:

```sh
./target/release/piair -v stream-test device:112233445566 \
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
