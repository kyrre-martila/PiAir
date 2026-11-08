# AirPlay 2 session and audio transport

## Implemented profile and evidence

PiAir now implements an explicitly invoked **AirPlay 2 realtime UDP / persistent
HAP / NTP / ALAC 44100 Hz, 16-bit stereo / 352 sample frames per packet** sender.
The original ALAC encoder, pairing, authorization and encrypted control work is
preserved. This profile is distinct from buffered TCP type 103 and legacy
ANNOUNCE/RSA/AES RAOP. The preserved type-103 components remain disconnected.

[AIRPLAY_REFERENCE_ANALYSIS.md](AIRPLAY_REFERENCE_ANALYSIS.md) pins the inspected
OwnTone and pyatv sender revisions, functions, differences and licenses. OwnTone's
actual active sender uses type 96/UDP, not buffered type 103. Its NTP configuration
provides the complete chosen sequence. pyatv independently corroborates HAP,
encrypted events, NTP, media-key offering, authenticated packets and feedback;
its PCM codec offer is not used. No sender implementation code is copied or
mechanically translated. No GPL component is linked into PiAir.

**Hardware compatibility and audible playback remain unverified.** Local mocks
exercise the production sender, not a separate protocol or a claim that an Apple
TV will play. A receiver that requires PTP, transient/Home membership, password,
MFi/FairPlay or another codec fails explicitly. No automatic transport fallback.

## Approval and cryptographic ownership

Only explicit commands connect. The exact approved selected device identity,
persisted discovery selection and current allowlist are checked by `Wire::connect`
before one bounded TCP attempt. Pair-verify validates the enrolled receiver ID,
Ed25519 signature, fresh contributory X25519 exchange, AEAD and M4. Credentials
prove enrollment; they are never interpreted as live authentication.

The same socket transfers into encrypted control ownership. Keys and raw sockets
are private. Separate event keys are derived during verify and can be consumed
only once by the event-channel owner. Media keys are fresh OS-random 32-byte
values per negotiated session, not HAP control/event keys or the verify secret.
The offered `shk` is transmitted only inside encrypted SETUP. Secret-bearing
serialized bodies and key material are zeroizing; temporary plist Data values are
wiped after serialization. Existing process-memory/local-user limitations in
PAIRING.md still apply; complete memory erasure is not claimed.

Additional event, timing, data and control endpoints are created in `wire.rs`
only after authenticated response validation. Their IP is always the live
verified TCP peer IP; a response supplies only checked nonzero u16 ports. URLs,
redirects, hostnames and address lists cannot redirect a connection. IPv4 only
for this initial diagnostic. UDP sockets bind to the actual control connection's
local interface; both IP and port are checked on received datagrams, including
anything queued before UDP connect.

Approval/selection are reloaded before every control operation, media send, sync,
event acknowledgement and timing reply. Revocation closes the session and
prevents remote cleanup traffic to the now-unapproved peer. There is no instant
watcher during a single bounded operation and no reconnect/resurrection loop.

The earlier blanket refusal of discovery bit 51 is refined: that capability does
not establish mandatory MFi. Pinned OwnTone uses ordinary persistent HAP without
its disabled auth-setup path. An explicitly selected persistent method still
requires bit 46; bit 51 alone grants nothing. MFi fields/flags, unsupported methods,
password/Home restrictions and invalid signatures remain errors; no fallback or
attestation bypass is introduced.

## Control and event encryption

Control uses HKDF-SHA512 with Control-Salt and directional
Control-Write-Encryption-Key / Control-Read-Encryption-Key labels. Each frame is
`[u16 LE length][ciphertext][16-byte Poly1305 tag]`; the length is AAD and the nonce
is four zero bytes followed by a per-direction u64 LE counter. Frames contain
1..1024 bytes, headers/bodies at most 8192 bytes each. Counter exhaustion, bad
tags, framing, status, correlation or I/O failure poison and close the wire.
There is no plaintext reentry, retry or nonce reset on an active connection.

Events use Events-Salt with **sender transmit Events-Read-Encryption-Key and
sender receive Events-Write-Encryption-Key**, independently counted from control.
The event worker accepts bounded `POST /command RTSP/1.0` binary-plist updateInfo
messages, validates framing/CSeq/type/value, and sends an encrypted correlated 200,
including CSeq zero. Unknown requests, invalid tags, termination or closure fail
the session. Remote play/pause commands are not implemented or silently accepted.

A control request has a 15-second absolute monotonic budget and at most a
three-second wait per partial I/O; event messages have a three-second budget.
Event/timing workers use bounded receive waits and are stopped/joined on cleanup.

## Typed negotiation and session state

Startup uses production encoders/decoders and this fixed sequence:

1. Fresh HAP pair-verify, then encrypted `GET /info`. Require audio/ALAC feature
   evidence and reject an advertised supportedFormats.audioStream mask excluding
   ALAC 44100/16/stereo. No codec guessing or fallback.
2. Bind local UDP timing/control/data sockets. For playback, start the existing
   peer-IP-filtered NTP responder and confirm worker readiness before initial
   SETUP. Advertise its actual bound local timingPort in the four-field NTP
   offer, together with the persistent sender deviceID and fresh sessionUUID.
3. Validate nonzero eventPort and, when present, timingPort (integer 1..65535)
   from the authenticated initial NTP response. Reject explicit PTP/None timing.
   Connect the encrypted event channel. No receiver timingPort is required or
   substituted: valid NTP probes from the verified receiver IP receive replies
   at their UDP source port throughout negotiation, playback and TEARDOWN.
   Session owns pending sockets/worker until transfer into MediaIo; failed
   negotiation retains them until best-effort TEARDOWN and joined cleanup.
   Setup-only A retains deferred timing, B/C retain early timing and C's
   pre-verify info order. See [playback lifecycle migration](REALTIME_NTP_LIFECYCLE.md).
4. Send RECORD and validate any Audio-Latency header within 11025..88200 samples.
   Absent latency uses the sender default 11025; zero is rejected. These are
   current sender policy bounds, not established protocol requirements; RECORD
   precedes the stream offer. See [latency evidence](RECORD_LATENCY.md).
5. SETUP one stream: type=96, audioFormat=0x40000, ct=2, spf=352, sr=44100,
   isMedia=true, latencyMin=11025, latencyMax=88200, local controlPort, independent
   shk, session streamConnectionID, supportsDynamicStreamID=false. Validate one
   returned stream, exact type and any echoed format/rate/latency fields, streamID
   and dataPort/controlPort. Connect UDP only to those peer-bound ports.
6. Set receiver volume to -30 dB using SET_PARAMETER text/parameters. Require a
   successful NTP query/reply and initial encrypted event acknowledgement before
   READY. Then send initial sync and the first authenticated audio datagram;
   only a successful send enters STREAMING.

URI is a fresh sender-address/numeric-session-ID RTSP URI; CSeq continues from
pairing. Responses must be RTSP/1.0 200 with matching CSeq. A returned Session
header is validated, pinned and included in subsequent requests; changes fail.
Optional streamID is range checked and correlated in feedback. Binary-plist
preflight rejects oversized/cyclic/deep/excessively expanding graphs before the
general parser allocates; total traversal <=2048 nodes, depth <=24, collection
size <=256. Unsupported binary object families and XML are rejected.

The lifecycle is DISCONNECTED → CONNECTING → AUTHENTICATING → NEGOTIATING → READY
→ STREAMING → STOPPING → DISCONNECTED. Any failure enters FAILED with the first
reason preserved and releases all resources. FAILED cannot reconnect. An explicit
new start after successful stop creates fresh authentication, identifiers, media
keys and counters. Repeated stop is safe.

## Audio packetization and media protection

The source for this milestone is deterministic stereo S16 **at 44100 Hz**. There
is no automatic ALSA adapter or resampling of the default 48 kHz capture. ALAC
escape coding supplies genuine CPE/partial-count/escape/END framing, without
predictive/entropy compression. Independent Symphonia decoding recovers exact
samples at the selected rate. No raw PCM is mislabeled ALAC/AAC.

Each UDP datagram is:

`[12-byte RTP PT96 header][ALAC ciphertext][16-byte tag][8-byte nonce]`

Header fields are version 2, first-packet marker, 16-bit big-endian sequence,
32-bit big-endian sample timestamp and random SSRC. One packet advances timestamp
by 352; both sequence and timestamp wrap normally. Timestamp+SSRC (header bytes
4..11) are AAD. ChaCha20-Poly1305 uses a fresh independent media key and nonce
`zero32 || u64 LE packet counter`; the last eight nonce bytes are transmitted.
The nonce is independent of sequence and never resets on wrap or FLUSH. Exhaustion
poisons the encoder. Unlike the inspected OwnTone u16 sequence nonce, this does
not repeat after sequence wrap; pyatv independently corroborates counter nonces.

One source block is generated on demand; there is no unbounded producer queue.
The resend cache holds at most 256 authenticated datagrams (~372 KiB). The older
48 kHz/type103 queue is not used by this realtime sender.

## Timing, latency, feedback and recovery

A wall-clock NTP epoch is sampled once and advanced by monotonic elapsed time,
avoiding later wall-clock steps. Timing d2 queries receive d3 replies with echoed
origin and measured receive/transmit timestamps. Only the authenticated peer's
negotiated port can obtain a reply. NTP traffic is protocol-defined plaintext;
there is no plaintext control/media downgrade. At least one timing exchange is
required before READY; silence over five seconds fails liveness.

The sample timeline uses total frames/44100, not summed rounded packet durations.
Audio deadlines are absolute from one Instant epoch. Waits are computed from the
next sample deadline while servicing feedback/workers. A scheduler gap of 100 ms
fails rather than producing an unbounded catch-up burst or queue. Deterministic
source and sender clock share the same monotonic rate; there is no capture clock
or adaptive resampler here, and no claim of correcting an independent ADC clock.

NTP sync d4 packets go to the negotiated control port initially and about once
per sample second. They map rendering RTP position, NTP time and next RTP sample.
As in OwnTone's NTP model, a two-second total output lead is represented by a
sync gap of 88200 minus the accepted receiver latency; this profile offers
11025 minimum and 88200 maximum. The diagnostic sends two seconds of silence
following the signal to drain latency before explicit stop. Exact physical
render latency remains hardware dependent. PTP d7 and buffered rate anchors are
not mixed into this NTP profile.

POST /feedback runs every two seconds and must succeed, with bounded valid
stream feedback if present. Eight-byte 0x80d5 loss requests specify a first u16
sequence/count. Up to 32 packets per request, three retries per cached packet and
1024 total requested packets per diagnostic are allowed. Cached ciphertext is
resent unchanged on the data UDP socket (the inspected OwnTone behavior), not
re-encrypted. Out-of-window packets are unavailable; no retry/reconnect loop.
Media/event/control loss, malformed feedback or worker failure closes the session.

## FLUSH, teardown, partial setup and diagnostics

FLUSH uses RTP-Info with the next sequence/timestamp. It clears resend history and
marks the next packet as a new start, without resetting keys/nonces. Explicit
stop then sends empty binary-plist TEARDOWN for the whole session, before closing
media, workers, event and control ownership. Any attempted session SETUP is
recorded so even a schema/port failure attempts bounded best-effort TEARDOWN if
the authenticated control channel and approval remain valid. A framing or I/O
failure may make remote teardown impossible; local cleanup still occurs.

`piair --config config.toml stream-test device:aabbccddeeff --seconds 1`
requires the currently approved selected receiver, existing credentials, a real
interactive terminal and exact-ID confirmation. Duration is 1..3 seconds of
signal plus two seconds drain; signal amplitude is bounded around -18 dBFS and
receiver volume is set to -30 dB. The low receiver volume may persist after the
test; the prompt says so. Authentication, READY, packets/retransmits/timing and
acknowledged teardown are reported separately. Ctrl+C signals cancellation and
attempts bounded authenticated teardown, then closes resources; a blocked I/O
still respects its existing timeout. Failure reports transmission
counts and cleanup attempts. **Audible playback always remains unverified unless
a person physically confirms it.** No hardware is contacted during development
or automated tests. No startup, status, config, discovery or audio activity path
calls the diagnostic.

## Validation boundaries and next integration

Loopback integration executes production verify, encrypted control/event I/O,
authenticated port binding, SETUP/RECORD/volume, NTP, actual UDP media, feedback,
loss/retransmit, FLUSH and TEARDOWN. It independently decodes ALAC and checks the
signal, RTP progression, nonce counter and sync latency fields. Negative paths
cover partial setup, bad codec/port/event tag, missing timing, live approval
revocation, disconnect and repeated sessions. Existing tests retain identity
mismatch, authorization/selection changes, control framing/replay and no-connect
read-only coverage. Python plistlib/cryptography fixtures validate the stream
offer and exact RTP/AEAD bytes; these are synthetic fixtures, not Apple TV captures.

Remaining unsupported behavior has concrete source boundaries: OwnTone's separate
PTP path uses libairptp and d7 sync, not this NTP path; the pinned sender has no
active buffered103 or rate-anchor implementation to reproduce. Future PTP or
buffered support requires its own profile and tests, not relabeling these UDP
messages. Event variants, NTP acceptance and latency on particular Apple TV or
HomePod firmware require hardware evidence. No manufacturer compatibility claim.

Future capture/activity integration must explicitly validate/resample the actual
ALSA rate to 44100 for this profile, propagate discontinuities, preserve bounded
pre-roll/backpressure and retain the same authorization owner. It must not simply
connect the default 48000 Hz detector stream or wire silence events into startup.
The planned capture integration remains a separate milestone.

## Realtime negotiation failure diagnostics

The reported Apple TV failure after successful HAP pair-verify and encrypted
OPTIONS does not establish which subsequent request stalled. No hardware was
contacted for this investigation. Negotiation failures now include a local stage
label and RTSP method for GET /info, initial SETUP, RECORD, stream SETUP and
SET_PARAMETER. Separate labels cover media socket setup, NTP worker startup,
event connection and the final NTP/event synchronization wait. Request errors
identify writes versus response reads/validation; response reads identify headers
versus body. Socket WouldBlock (including Linux EAGAIN/os error 11 on a blocking
socket with a read timeout) and TimedOut are reported as read timeouts, separately
from other read errors. Validated numeric receiver rejection statuses and status
200 on subsequent response validation failures are retained. Worker errors name
the event or NTP worker; synchronization timeouts report only readiness booleans.
The command's existing error log emits this context at default verbosity.

Diagnostics never include request URIs containing session IDs, PINs, credentials,
keys, tokens, payloads, raw plists, headers or receiver metadata. Binary plist
library errors are replaced with a fixed message rather than forwarding decoder
error content. Stage labels and method names come from local constants.

The request sequence remains GET /info, initial SETUP, authenticated event
connection with NTP service running, RECORD, stream SETUP, volume SET_PARAMETER,
then NTP/event readiness. The existing 15-second request budget, capped 3-second
socket reads/writes, and 3-second synchronization wait remain unchanged. There
is no evidence here to justify a protocol ordering change or longer timeouts.
Failure cleanup limits best-effort TEARDOWN to a 250ms request budget, preserves
the original failure, then closes control/event sockets and joins workers. The
NTP worker has 100ms read and write timeouts; shutting down the event socket
interrupts partial frame reads. Cleanup errors do not replace the primary error.

Loopback mock regressions cover timeouts, rejected statuses and malformed
responses at each RTSP negotiation request; failed event connection; malformed
stream/session responses; missing NTP or initial event; NTP/event worker failures;
a receiver ignoring TEARDOWN; partial encrypted event cleanup; and worker panic.
They check attribution, sensitive-text exclusion, terminal state, zero audio
packets, channel closure and idempotent stop. Existing allowlist, authorization,
explicit confirmation, encryption and response validation remain enforced.

Unverified on Apple TV: acceptance of the existing realtime NTP/ALAC profile and
request sequence, arrival of the initial updateInfo event and NTP queries within
the existing deadlines, and successful playback. Mock tests validate local
behavior, not those hardware assumptions. No OwnTone, Autostream or system
services were changed.

### Initial SETUP sender headers (PR #10)

Authenticated session requests now include matching hex Client-Instance/DACP-ID,
a decimal u32 Active-Remote and X-Apple-Client-Name: PiAir, following the inspected
audio senders. IDs are random per control connection and stable across requests.
The NTP plist, IPv4 session URI, protocol version, encryption and ordering remain
unchanged. Request errors distinguish construction from encrypted writes.
[Initial SETUP research](INITIAL_SETUP_RESEARCH.md) contains immutable upstream
source links, independent fixture details, compatibility limits and safe operator
verification instructions. AppleTV6,2/tvOS 26.6.1 acceptance remains unverified.
