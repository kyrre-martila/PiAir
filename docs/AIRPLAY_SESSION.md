# AirPlay session and transport

The production live path uses realtime PTP/type-96. The NTP diagnostic path
retains bounded tone/WAV tests. Both enforce explicit approval, authentication
and cleanup. Buffered live playback is unsupported. See [architecture](ARCHITECTURE.md)
and [runtime](RUNTIME.md) for production orchestration.

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
The event worker accepts bounded `POST /command RTSP/1.0` binary plists, validates
framing/CSeq/type and known values, and sends an encrypted correlated 200, including
CSeq zero. updateInfo dictionaries remain supported. sendMediaRemoteCommand strings
and unknown event types are explicitly ignored and acknowledged, without playback
actions. Malformed requests, invalid tags and unexpected closure fail the worker.
An established idle event channel is healthy; see the developer guide.

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
   pre-verify info order. See playback lifecycle migration.
4. Send RECORD; treat Audio-Latency as informational, including zero, absent,
   malformed and out-of-offer values. Debug diagnostics expose only typed numbers
   and fixed parse status. Sender timing is independent of this header.
   See latency evidence and sender model.
5. SETUP one stream: type=96, audioFormat=0x40000, ct=2, spf=352, sr=44100,
   isMedia=true, latencyMin=11025, latencyMax=88200, local controlPort, independent
   shk, session streamConnectionID, supportsDynamicStreamID=false. Validate one
   returned stream, exact type and any echoed format/rate/latency fields, streamID
   and dataPort/controlPort. Connect UDP only to those peer-bound ports.
6. Set receiver volume to -30 dB using SET_PARAMETER text/parameters. Require a
   successful NTP query/reply and a healthy established event channel before
   READY; no unsolicited event is required. Then send initial sync and the first
   authenticated audio datagram;
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
IP can obtain a reply, sent to the timing probe's source port. NTP traffic is protocol-defined plaintext;
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
fixed sender sync gap of 77175 samples (1750 ms), plus the separately offered
11025-sample stream component. The offer maximum remains 88200. RECORD headers
never change this calculation. OwnTone defaults to a different total buffer
(2250 ms); see the complete calculation. The diagnostic sends two seconds of silence
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
