# AirPlay session foundation

## Status and scope

PR #4 uses the requested **verified-components fallback**. It implements a live
persistent HAP authenticated control owner and real encrypted RTSP OPTIONS I/O.
Prompt 4b adds independently validated
ALAC encoding and receiver-observed packet-protection components without
enabling a network audio profile.
**It does not implement a complete buffered audio transport profile.** No audio
is transmitted, even to the mock receiver. No Apple TV or HomePod model/firmware
has been validated. READY and STREAMING are deliberately unreachable.

The base is fresh main at `62f28d879789ee0c7ee5e7cd66d5801a0f93f02e`, the merge
commit of PR #3 (GitHub reported merged at 2026-11-07T09:20:43Z). PRs #1–#3 are
in its history. All additions are original; no sender implementation code is
copied, translated or incorporated. No new runtime crate is needed; Symphonia
is a test-only independent ALAC decoder (see DEPENDENCIES.md).

## Evidence and transport profile research

Reviewed 2026-11-07. Sources describe different profiles; they cannot be combined
into an assumed successful Apple TV session.

| Reference | Evidence and consequence |
| --- | --- |
| [HAP R2 §6.5.2, public mirror](https://forum.iobroker.net/assets/uploads/files/1634848447889-apple-spezifikation-homekit.pdf) | Normative HAP IP session-security framing and directional key derivation after pair-verify. Implements control protection, not media protection. |
| [Independent AirPlay HKP notes](https://openairplay.github.io/airplay-spec/pairing/hkp.html) | Corroborates AirPlay's switch to encrypted control on the existing connection and separate keys/counters. Does not establish buffered playback. |
| [AirPlay 2 Internals: RTSP](https://emanuelecozzi.net/docs/airplay2/rtsp/) | Documents binary plist control, a two-stage SETUP, event channel and timing selection; distinguishes type 96 realtime and type 103 buffered streams. The worked audio example is type 96 ALAC, not a complete 48 kHz buffered profile. |
| [AirPlay 2 Internals: RTP](https://emanuelecozzi.net/docs/airplay2/rtp/) | Describes a 12-byte RTP header, timestamp/SSRC AAD, sender-generated `shk`, ChaCha20-Poly1305 and a media nonce/tag trailer. The illustrated header is type 96. This does not validate type 103 framing, counter lifecycle or receiver-specific key requirements. |
| [AirPlay 2 Internals: encryption](https://emanuelecozzi.net/docs/airplay2/encryption/) | Distinguishes outer control protection from encrypted media, including observations with outer encryption disabled. PiAir never attempts such a downgrade. |
| [pyatv protocol documentation: AirPlay 2](https://pyatv.dev/documentation/protocols/#airplay-2) | Explicitly incomplete; its detailed flow is remote-control-only (type 130). Event/data channel keys and `timingProtocol=None` in that example cannot be repurposed as audio transport or synchronization. It also reports control blocks larger than HAP's limit; PiAir conservatively rejects blocks over 1024. |
| [Legacy SETUP](https://openairplay.github.io/airplay-spec/audio/rtsp_requests/setup.html), [RTP streams](https://openairplay.github.io/airplay-spec/audio/rtp_streams.html) | UDP AirTunes examples, RTP sequence/sample timestamps, NTP sync and retransmission. They are not sufficient evidence for buffered TCP audio. |
| [FLUSHBUFFERED](https://openairplay.github.io/airplay-spec/audio/rtsp_requests/flushbuffered.html) | Buffered flush uses binary plist sequence/timestamp ranges, unlike legacy FLUSH's RTP-Info header. |
| [RFC 2326](https://www.rfc-editor.org/rfc/rfc2326), [RFC 3550](https://www.rfc-editor.org/rfc/rfc3550), [RFC 8439](https://www.rfc-editor.org/rfc/rfc8439) | RTSP correlation/body framing, RTP fundamentals and the established AEAD primitive. These do not specify Apple's buffered profile. |

The Internals site returned HTTP 502/503 directly during research; indexed
versions of its RTSP/RTP/encryption pages were available. Treat those observations
as incomplete research evidence. No receiver capture was made. Sender-project
search excerpts were not treated as independent conformance evidence or used as
implementation code.

## Prompt 4b profile decision

[BUFFERED_RESEARCH.md](BUFFERED_RESEARCH.md) records the new evidence and scope
before component implementation. An independent receiver parser resolves a
specific buffered layout: 24-bit sequence (no normal RTP PT byte), timestamp,
format word, ciphertext/tag/nonce and a two-byte TCP length counting itself.
This is **not** a complete modern Apple TV profile. ALAC codec validity is now
verified; its acceptance, format-word semantics, key exchange and PTP/anchor
requirements remain unestablished. No SETUP/RECORD/FLUSH/TEARDOWN sequence or
receiver-directed streaming diagnostic is enabled. The existing encrypted
control owner and authorization boundary are preserved.

## Authentication-to-session transition

`Session::start` progresses DISCONNECTED → CONNECTING → AUTHENTICATING →
NEGOTIATING. Production supplies `Wire::connect`, the sole receiver TCP boundary.
That boundary reloads approval and pinned selection, compares the discovery
snapshot and rejects ambiguous identities/endpoints before one connect attempt.
There are no retries, DNS resolution, alternate addresses or redirects.

`pairing::verify` consumes the wire and checks the enrolled receiver identifier,
Ed25519 signature, fresh contributory X25519 shared secret, M2 AEAD and M4 success.
It then transfers that same wire into encrypted mode with derived keys; neither
socket nor secrets escape the owner. Approval and persisted selection are checked
again at handoff. A credential file proves enrollment, never current liveness or
active session authentication. Credentials and ephemeral control keys are separate.

The session can retain the connection for repeated explicit control checks.
OPTIONS success leaves it NEGOTIATING; it proves neither usable codecs nor media
ports, readiness or playback. Calling the internal `start_buffered_audio` gate
fails explicitly and closes control without sending SETUP or media.

| State | Entry condition / exit |
| --- | --- |
| DISCONNECTED | New owner or successful local shutdown; explicit start only |
| CONNECTING | Authorization boundary invoked; error → FAILED |
| AUTHENTICATING | Connected wire consumed by pair-verify; error → FAILED |
| NEGOTIATING | Verified M4 and approval recheck; encrypted OPTIONS permitted |
| READY | Reserved; no implemented SETUP can enter it |
| STREAMING | Reserved; no implemented RECORD/media path can enter it |
| STOPPING | Synchronous local resource cleanup |
| FAILED | First failure reason retained, socket/keys dropped; no restart on this owner |

Shutdown/drop is idempotent. No audio session was created, so local shutdown does
not send a speculative TEARDOWN. A future implemented SETUP must add appropriate
remote cleanup, including cleanup following partial setup failures.

## Control-channel encryption

HKDF-SHA-512 over the fresh verify shared secret uses `Control-Salt` and
`Control-Write-Encryption-Key` (controller transmit) or
`Control-Read-Encryption-Key` (controller receive). Neither derives a media key.
Each frame is `[u16 LE plaintext length][ciphertext][16-byte Poly1305 tag]`.
The two-byte length is AAD. The nonce is four zero bytes followed by the direction's
u64 LE counter. Both counters start at zero and advance independently per frame;
the implementation rejects exhaustion before wraparound. Keys are never cloned
or used with another socket. No plaintext POST is permitted after the upgrade.

Production I/O handles partial reads/writes, arbitrary TCP fragmentation and RTSP
headers crossing encrypted frame boundaries. It bounds frames to 1..1024 bytes,
headers to 8192 and bodies to 8192 bytes. A diagnostic request has a 15-second
absolute monotonic deadline, recalculated before each partial I/O, and a maximum
three-second wait per read/write. There is one in-flight request. CSeq continues
from pairing and is mandatory/matched for encrypted replies; only RTSP/1.0 200
is accepted. RTSP's omitted Content-Length means zero; explicit invalid lengths,
duplicates, redirects, transfer encodings, connection-close and unsolicited
messages fail. Unexpected OPTIONS bodies or buffered trailing plaintext fail.

Framing, I/O, tag, counter, authorization or protocol failure closes the socket,
drops zeroizing keys and clears the fixed plaintext frame buffer. No retry,
plaintext fallback or counter reset on a live connection is allowed. Stalls and
EOF during an operation are terminal. There is no background idle socket monitor:
idle peer closure or revocation is detected at the next operation (or explicit
shutdown). Approval/selection are rechecked before and after each diagnostic;
there is no instant revocation watcher during an in-flight bounded operation.
Secret-copy/allocator and same-user-process limitations in PAIRING.md still apply.

## Audio components; network streaming disabled

The required input contract is stereo 48,000 Hz signed 16-bit PCM, ordered left /
right sample frames. This is an input requirement, **not** an asserted AirPlay
wire codec. Existing capture normalizes native interleaved S16/S32 to indexed
`detector::Frame` values and can negotiate rates other than 48 kHz. No adapter
currently forwards those values to this session.

No wire codec is negotiated. `audio::alac` independently implements stereo
48 kHz / S16 ALAC escape frames, decoder configuration and checked normalized
sample conversion. It does not merely relabel PCM: the output has CPE/escape/
partial-count/END syntax and is decoded bit-exactly by Symphonia in tests.
There is no predictive/entropy compression. Blocks contain 1..4096 sample frames;
maximum encoded size is bounded by 16,392 bytes.

`receiver::media` supplies components with no sockets, key export/exchange or
session negotiation: a bounded ordered S16 queue, absolute sample-duration
arithmetic and an observed-layout packetizer using fresh OS-random independent
media keys. Its sequence field is 24 bits, timestamp is 32 bits, and nonce is an
independent 64-bit LE counter. Sequence/timestamp wrap cannot reset the nonce.
AEAD uses timestamp/format-word bytes as AAD and appends the tag then eight-byte
nonce; TCP framing adds a big-endian length counting itself. Validation or nonce
exhaustion poisons the packetizer. No HAP secret is accepted as its key input.

This packet model matches the pinned receiver parser in BUFFERED_RESEARCH.md;
its field/reserved-bit meanings and wrap convention are not verified Apple TV
behavior. The supplied opaque format word is not a negotiated codec identifier.
Components have no caller on a receiver-directed path, are compiled under a
narrow dead-code allowance and are exercised by deterministic component tests.
No media is sent to a receiver or socket, no media key is exchanged, and no
successful audio session can be inferred from these tests.

## Timing, feedback and recovery: disabled

The control deadline uses `Instant`, not sleeps. It is not a playback clock.
There is no NTP/PTP responder, rate-anchor exchange, receiver latency negotiation,
clock drift estimator, playout scheduler or feedback worker. The new sample
timeline derives elapsed time from total 48 kHz sample count
without accumulating per-packet rounding; it is not clock drift correction.
Existing detector sample indices are local capture time only. Legacy NTP sync packets and UDP
retransmission queries must not be assumed to apply to type 103 TCP transport.
The audio queue bounds capacity to at most 48,000 stereo frames (one second);
it refuses full writes atomically, without eviction or index advancement. Its
capacity is a local safety bound, not negotiated receiver latency. There is no
retransmission buffer, keepalive loop or reconnect loop.
Receiver-initiated requests on the control socket fail as unsupported protocol;
no event channel has been negotiated. Idle event-channel termination is untested.

## Precise blockers before enabling buffered audio

1. Establish a coherent authenticated type 103 transcript for the allowed Apple
   TV firmware/profile: `/info` formats, complete two-stage SETUP fields, event
   channel behavior, session/stream identifiers and port validation, RECORD,
   pause/flush/rate-anchor and TEARDOWN, including partial-setup rollback.
2. Validate a stereo 48 kHz wire format and its advertised format-bit/`ct`/`spf`
   mapping. Implement the actual codec and framing it needs. ALAC support must
   follow its specification; changing an identifier around raw PCM is invalid.
3. Confirm buffered packet framing, exact media AEAD AAD and tag/nonce placement,
   nonce endianness/width and long-session uniqueness against independent Apple
   TV observations. A pinned independent receiver now confirms tag-before-nonce
   and 24-bit buffered sequence, superseding the older realtime diagram for our
   component tests. It does not establish Apple TV acceptance/reserved-bit rules.
   A wrapping sequence alone cannot be a safe long-session nonce.
   Validate `shk` acceptance/key protection separately from HAP. Required FairPlay
   or MFi exchange must remain an explicit unsupported error, never a bypass.
4. Validate the receiver's PTP/NTP dialect and rate-anchor requirements, map a
   monotonic sample epoch to that clock, bound drift and latency, and test the
   sequence/timestamp wrap rules. A remote-control `None` timing profile or fixed
   sleeps cannot supply playback synchronization.
5. Validate buffered feedback, flow-control/buffer limits, stalls and transport
   reliability semantics. Determine whether any application retransmission is
   required rather than copying legacy UDP recovery into TCP.

These are gaps in PiAir's evidence and implementation, not a claim that the
protocol is impossible or that no other project implements it. Current approval
policy still excludes unresolved bit-51, transient, password/Home-membership,
legacy and FairPlay profiles. HomePod transient pairing remains out of scope.

## Component validation added in Prompt 4b

Tests pass real encoded packets through authenticated media decryption and an
independent Symphonia ALAC decoder, preserving the deterministic stereo signal.
They check partial/full codec blocks, signed extremes, conversion rejection,
queue backpressure/gaps/limits, sample-duration rounding, observed sequence and
sample-timestamp wrap, independent nonce progression/exhaustion, key invalidation,
fresh keys and tamper rejection. A Python cryptography 50.0.0 fixture validates
packet bytes independently of RustCrypto composition. The fixture is synthetic,
not a hardware capture; this is component validation, not a full session mock.
No Python, FFmpeg or external executable is required by cargo tests.

## Tests and PR #5 integration contract

Loopback tests run the production persistent verify exchange, control framing,
RTSP parser, authorization boundary and state owner. They cover repeated encrypted
OPTIONS with differing transmit/receive frame counts, fragmented headers/lengths/
tags, independent fixtures, nonce exhaustion/replay, invalid statuses/CSeq/lengths,
plaintext rejection, identity mismatch, approval revocation during verify and
before requests, selection changes, disconnects, a stalled operation and repeated
shutdown. Existing tests cover read-only dispatch with an approved selected
loopback endpoint; no receiver connection occurs. No tests contact real receivers.

`tests/fixtures/hap-control.hex` is generated independently using Python
cryptography 50.0.0 (HKDF-SHA512/ChaCha20Poly1305), with public synthetic inputs and
counter 0/1 in both directions. The optional generator is included; cargo reads
static bytes and needs no Python dependency. This is independent composition
validation, not a captured Apple TV transcript.

PR #5 must not wire activity events or ALSA into networking until the blockers
above are resolved. Keep `audio` independent of receiver connections. The future
adapter must validate the actual capture rate, convert explicitly to the verified
codec input, preserve ordered frame indices within one epoch, reject discontinuity,
and propagate bounded backpressure instead of growing or silently dropping audio.
Provide deterministic frames to that same adapter first; a mock must use production
media encoders and the state machine before any claim of encrypted audio success.
Any eventual new event/media connection must go through the approval boundary
with validated negotiated ports and authenticated identity binding. No raw socket
or key API should be exposed to capture/activity code.
