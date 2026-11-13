# PTP realtime WAV playback (PR 63)

## Implemented architecture

The explicit `ptp-wav-playback` command uses the selected, approved receiver and
terminal y/yes confirmation, as `stream-test` does. StoredHap is the default;
transient authentication requires its existing explicit experimental flag.
There is no protocol fallback. Omitting `--volume-db` preserves receiver volume;
supplying it authorizes a persistent receiver volume change (-60 through -10 dB).
The bounded WAV duration defaults to 30 seconds, accepts 1–49 seconds, and stops
at EOF. WAV parsing/resampling happens before authentication, using the existing
reader, so file I/O never blocks the PTP worker.

The proven PR 62 session remains the sole authenticated owner. Startup is:

1. Authenticate on the selected connection using stored HAP, or explicitly
   selected OwnTone-reference transient SRP, and transfer that Wire once.
2. Authenticated `/info`, initial PTP SETUP, RECORD, SETPEERS, type96 stream SETUP.
3. Validate session binding, negotiated ephemeral UDP ports and stream profile.
4. Wait at most five seconds for healthy local timing and live authorized peer
   traffic. This requires no undocumented proof of receiver clock adoption.
5. Set receiver volume only if explicitly requested, on the same authenticated
   session. Validate the response and session binding.
6. Send initial PT215 sync immediately before the first encrypted audio packet;
   pace WAV packets and continue periodic sync and feedback.
7. Stop transmission, join the media-period timing and feedback workers, drop
   media sockets/key/cache, perform bounded authenticated TEARDOWN and release
   timing/event/control resources. Failure and cancellation use the same cleanup.

The profile is **type96 UDP, ALAC16 stereo, 44,100 Hz, 352 samples per packet,
ChaCha20-Poly1305, PTP**. No buffered type103, TCP media, ALAC24/48k, or
SETRATEANCHORTIME is involved. Legacy buffered diagnostics keep their separate
closed media gate. Existing NTP CLI commands and authentication are preserved.

`receiver/playback.rs` supplies one WAV pacing loop for both NTP and PTP outputs.
Both use the existing WAV reader, ALAC encoder, RTP packetizer, nonce state,
bounded 256-packet ciphertext history, and feedback worker. The existing NTP
diagnostics keep their stricter retransmission policy. WAV retransmission
uses the exact cached encrypted datagram, with no second encryption operation.
Requests are peer-bound and size/count checked; out-of-window packets are skipped,
retries are capped at three per cached packet and 1,024 requested packets per run.
Malformed requests and exhausted budgets fail with cleanup.

## One monotonic timeline

The scheduler uses absolute `Instant` deadlines calculated from total transmitted
samples / 44,100, rather than accumulated sleeps. PTP's existing grandmaster maps
those same scheduled instants through its original monotonic epoch. No wall-clock
UTC origin or second PTP clock is introduced. Worker health is checked at actual
current time; scheduled sync timestamps are converted using the owned epoch,
including when the timing worker has advanced beyond the scheduled deadline.

For the next RTP position N, the sync current position is N - 77,175; the PT215
next-position field is N - 11,025. Positions wrap modulo 2^32. Initial and periodic
syncs share the same sample positions as the ALAC packets, and carry this timing
service's ClockID and big-endian monotonic nanoseconds. The first sync starts
90 d7 00 06; later syncs start 80 d7 00 06; all are exactly 28 bytes. Periodic sync
is sent at the first packet boundary at least 44,100 samples after the preceding
sync (352-sample packet boundaries give approximately one second).

The sender retains the existing two-second lead/drain policy: transmit signal
followed by 88,200 silent frames. Full RTP packets always encode 352 frames;
EOF, a duration cutoff and the final drain block are zero-padded without reading
past the WAV or looping it. Packets have the existing 12-byte RTP header, initial
marker, wrapping sequence and sample timestamps, timestamp/SSRC AAD, encrypted
ALAC, 16-byte tag and eight-byte little-endian nonce suffix. Nonces increase
independently of sequence wrapping and are never reset by cache flushes.
The verified session's key is retained, never logged, and zeroized on teardown.
Cache bytes are erased when flushed/dropped.

A scoped worker polls PTP every two milliseconds while audio encoding, sending,
retransmission and asynchronous authenticated feedback run. Timing state is
briefly locked for checks/sync serialization, never across encoding or network
control requests. All workers join before cleanup; none detach. The scheduler
allows at most 100 ms lateness before failing rather than an uncontrolled burst.
Feedback is requested every two seconds over the same Wire, with its existing
500 ms budget, CSeq, directional keys and strict original PTP session binding.
Healthy PTP also continues during RTSP startup/volume/TEARDOWN requests. TEARDOWN
uses the existing 250 ms cleanup budget; revoked authorization forbids further
network writes and reports remote cleanup unconfirmed.

Diagnostics distinguish local transmission from audible output: receiver/profile,
RTSP stages, ephemeral media ports, PTP packet evidence and health, audio packet
and byte counts, retransmission request/response counts, PT215 sync count,
elapsed transmission time, completion/failure, and cleanup/TEARDOWN outcome.
A receiver response or successful UDP send is never reported as a listening test.

## Protocol reference and licensing

Before implementation, inspected current OwnTone HEAD
[`8ed84f2b3cba25d3de65e05660d59ce034b5adab`](https://github.com/owntone/owntone-server/tree/8ed84f2b3cba25d3de65e05660d59ce034b5adab):
`src/outputs/airplay.c`, `src/outputs/rtp_common.c`, `src/libairptp/` and
`src/ptpd.c`. Studied realtime packet/encryption format, initialization, sample
mapping, initial/periodic sync, buffering, scheduling, retransmission, startup
and shutdown. The RTP sync interval defaults to one sample-rate worth of frames;
PTP sync timestamps use the monotonic player time. OwnTone's encryption uses a
sequence-derived nonce; PiAir preserves its stronger existing independent u64
counter through RTP sequence wrap. No GPL code was copied, translated, linked or
vendored. Changes are original Rust under PiAir's Apache-2.0 license.

## Validation and remaining hardware work

PR 62's hardware baseline is established: Apple TV Stua authenticated, accepted
all realtime negotiation stages and exchanged healthy PTP traffic, with no audio
sent. That investigation is not repeated here. Receiver clock adoption remains
unverified and is not a media gate.

Tests include actual encrypted RTSP framing and session binding, ephemeral UDP
media/control sockets, initial/periodic 28-byte sync, real encrypted ALAC decoded
by the independent test decoder, sequence/timestamp wrap, unique nonces, exact
ciphertext retransmission, pacing, partial EOF and silent drain, cancellation,
timing loss, malformed requests, disconnect and released sockets. Deterministic
scheduler tests drive a virtual monotonic clock. Existing authentication, key
zeroization, packet layout, nonce exhaustion, WAV reader and HomePod NTP suites
remain required. No new audio fixture is added.

Physical listening on the Raspberry Pi and Apple TV is still required. This cloud
environment has no established access to either device. Local test success does
not verify clock adoption, output routing, receiver buffering or audible sound.

## Exact Raspberry Pi deployment and listening test

Under the same Pi user/configuration that paired and selected Stua:

```bash
cd ~/PiAir
git fetch origin feat/ptp-wav-playback
git switch --detach origin/feat/ptp-wav-playback
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
sudo setcap cap_net_bind_service=+ep ./target/release/piair
./target/release/piair -v ptp-wav-playback device:02d899c1833f \
  --audio-file tests/assets/audio/Pachelbel_Canon_audio_test.wav \
  --seconds 10 --volume-db -20
```

Confirm the interactive playback prompt with `y`. This requests approximately
10 seconds of music plus two seconds of silent drain. **Listen at the selected
Apple TV's actual output and record whether music was audible.** The explicit
-20 dB receiver setting can persist after playback/failure and is not restored.
These values are test arguments, never implementation defaults for a receiver.
Reapply setcap after rebuilding; UDP319/320 must be free on the selected local
interface. The approved, paired receiver must already be selected; do not change
its authentication method as part of a playback retry.

Record the CLI outcome, negotiated profile/ports, packet/retransmission/sync
counts, PTP health, elapsed duration and TEARDOWN. Separately record the listening
result, output route and any dropouts. Repeat with Ctrl+C, receiver disconnect
and timing loss to check prompt failure and local socket release. Re-run the
existing NTP HomePod WAV command unchanged as the hardware regression. Packet
captures may supplement failures, but cannot replace the listening result.


## Explicit HomePod transient test (PR 64)

`ptp-wav-playback --experimental-owntone-transient` requires the exact approved
receiver to be selected with `--method transient`. Without the flag, the command
requires a persistent selection and existing stored HAP credentials. A persistent
selection combined with the flag, or a transient selection without the flag,
fails locally. There is no automatic fallback or receiver switching.

The flag reuses `stream-test`'s `wav_authentication` policy and session SRP
implementation, including the OwnTone-reference RTSP `/info` preamble. The same
connection then supplies authenticated capabilities and negotiates initial PTP
SETUP, RECORD, SETPEERS and realtime type-96 UDP SETUP. Discovery advertisements
are only a prerequisite: authenticated capability checks and actual negotiation
must pass. Rejection fails with a stage-specific diagnostic and bounded cleanup;
an established session attempts authenticated TEARDOWN while the control channel
is usable. An RTSP rejection or disconnect can invalidate that channel, so remote
expiry remains unconfirmed and local resources are released. Failed authentication
closes the connection without attempting an unauthenticated TEARDOWN.

Transient authentication does not verify a persistent receiver identity and saves
no pairing credentials. Approval, exact selection, fresh discovery validation,
interactive consent, and ongoing authorization checks still apply. ALAC, RTP,
media encryption, PTP, pacing, latency, UDP and feedback are unchanged.

Deploy on the Raspberry Pi:

```sh
cd ~/PiAir
git pull
cargo build --release
sudo python3 scripts/grant-ptp-capability.py "$PWD/target/release/piair"
getcap "$PWD/target/release/piair"

./target/release/piair select device:0a05559f132f --method transient

./target/release/piair -v ptp-wav-playback device:0a05559f132f \
  --experimental-owntone-transient \
  --audio-file tests/assets/audio/Pachelbel_Canon_audio_test.wav \
  --seconds 15 \
  --volume-db -20
```

Confirm the interactive prompt. The receiver must already be approved. Explicit
volume changes can persist. This deployment sequence assumes the PR has been
merged into the Pi's current branch.

HomePod Stua advertised PTP and ALAC; that does not establish realtime type-96
compatibility or audibility. Its earlier audible transient NTP stream stopped
with `receiver NTP timing stalled` after roughly four seconds. The NTP timeout
logic is unchanged here. Apple TV Stua accepted authenticated PTP type-96 UDP,
but audible playback remains unconfirmed. Automated tests verify local protocol
and cleanup behavior; physical HomePod playback must be confirmed separately.

See [PTP port permissions](PTP_PORT_PERMISSIONS.md) for the capability security
scope, helper validation, and the required repeatable post-build step. Run PiAir
as kyrre; only the capability installation step uses sudo.

The optional `piair setup` test uses the same sender and existing recording for
49 seconds (or earlier EOF), preserving receiver volume. See [SETUP.md](SETUP.md)
for resource locations and the recording’s separate CC BY-SA 3.0 attribution.
