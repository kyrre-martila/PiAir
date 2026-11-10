# Buffered AirPlay 2 transport foundation

Current single-receiver status: [BUFFERED_LIFECYCLE.md](BUFFERED_LIFECYCLE.md)
implements an opt-in authenticated initial PTP SETUP diagnostic and bounded
cleanup. Live buffered playback remains closed because receiver clock adoption
is unverified. Historical PR #42 prerequisite lists below remain unresolved
except for that explicitly documented foundation. No verified audio is claimed.

Baseline: main / PR #39 merge `0608f2307a7c81b5faa5bd7a19f00607dbe04785`.
This implements independently testable type103 transport and a local encoder.
**No CLI or session can activate buffered playback. No audible playback is verified.**
The existing `Session::start_buffered_audio` gate remains closed.

## Architecture and ownership

`receiver/buffered/mod.rs` owns the TCP data socket, framing, media key, nonce,
23-bit sequence, 48 kHz sample timestamp, encrypted backlog and cleanup.
`buffered/codec.rs` defines `BufferedCodec` and an original stereo S24 ALAC escape
encoder. `buffered/tests.rs` contains device-free decoder, crypto, framing and
loopback tests. The module uses the existing RustCrypto primitive wrappers and
bounded plist serializer, but no realtime RTP/NTP transport, clock or encoder.
The older `media.rs` is a historical receiver-parser experiment, not the new
transport, format negotiator or a valid ALAC24 implementation.

`Prepared` generates a fresh random 32-byte media key. A single `setup_offer`
returns a zeroizing type103 plist for an authenticated control exchange. Move
that owner into `attach` after successful negotiation; drop it on setup failure.
It cannot be cloned, import a key, reset a nonce, or produce a second offer.
`NegotiatedStream::parse` checks a bounded, decoded same-session SETUP response:
exactly one type103 stream, nonzero u16 data port, matching optional format,
codec, frame-size, sample-rate, encryption and signed connection-ID echoes.

`attach` accepts an already connected socket; it does not dial or authenticate.
The future engine must obtain that socket through the authorization boundary,
using the approved literal receiver address and authenticated negotiated port,
with bounded connection time and cancellation. It must preserve same-receiver
and same-session binding of /info, SETUP, key and socket. No production caller
currently attaches a socket; tests use loopback only. This intentionally adds no
second production dialing path around `wire.rs`'s allowlist checks.

`enqueue` accepts exactly 352 right-aligned S24 stereo samples per channel.
One encrypted frame is the complete userspace backlog. A full backlog returns
false before encoding or advancing any counter. No eviction, hidden input queue,
background worker, retransmission queue or per-receiver thread is created.
`drain` uses nonblocking TCP writes and preserves the exact ciphertext suffix
across partial writes, Interrupted and WouldBlock. It checks cancellation and
an absolute deadline on each iteration, polling stalled writes every 2ms.
The effective deadline is the earlier of the caller's deadline and three seconds.
No progress can extend it. Kernel TCP buffering is also finite; TCP provides
flow control, not evidence that the receiver decoded or played a sample.

Encoding/encryption, nonce exhaustion and write failures poison the owner,
zeroize owned key/backlog and shut down both socket directions. Drop aborts;
explicit shutdown first drains under the same deadline and then closes.
There is no restart/reset/retry API. A new session needs a new `Prepared`.
Dropping a partially negotiated session remains the future control owner's
responsibility, including bounded RTSP TEARDOWN. Socket closure is not RTSP TEARDOWN.

## Protocol evidence and deliberate assumptions

Sources were inspected for facts; no GPL code was copied, translated, vendored,
linked or used as an implementation skeleton. This is not a claim of a
personnel-separated clean room. The immutable PR #39 source ledger remains in
[RECEIVER_PLANNING.md](RECEIVER_PLANNING.md).

| Evidence | Facts used / limits |
| --- | --- |
| [owntone-mini e9b0584](https://github.com/lo-tech-systems/owntone-mini/tree/e9b058448b7d921ed048790b45614685fd13802b), `src/outputs/airplay_buffered.c:45-185` | BE u16 length includes itself; 12-byte header; ciphertext, 16-byte tag, 8 transmitted nonce bytes. Header word is marker bit31 plus sequence bits0..22. Timestamp and format word are AAD. Source copies a native-endian u64 nonce; PiAir explicitly selects little endian, consistent with the independent receiver model and supported Pi targets. Big-endian sender behavior is not inferred. |
| Same revision, `src/outputs/airplay.c:117-154, 1726-1729, 4400-4440, 5215` | Format21 ALAC 48k/24 stereo, 352 samples, ct2, audioFormat mask `1<<21`, index21, payload format word `0x15000000`. SETUP supplies `shk`; RTP streamConnections explicitly uses it. Formats22/23 are AAC-LC stereo 44.1/48k, 39 surround. Signed 64-bit streamConnectionID is distinct from sequence/nonce. |
| [Independent receiver 6c343d3](https://github.com/openairplay/airplay2-receiver/blob/6c343d3679ddb561c61566985acaaf587d0a3bd3/ap2/connections/audio.py), `RTP`, `RTP_BUFFERED`, decryption and buffered read loop | Corroborates framing, total-length convention, timestamp/format AAD, trailing nonce and ChaCha20-Poly1305. It reads 24 sequence bits; PiAir uses the narrower 23-bit sender range and keeps the extra bit zero. It is not a timing-conformant receiver or Apple hardware evidence. |
| [Apple ALAC cookie description](https://github.com/macosforge/alac/blob/master/ALACMagicCookieDescription.txt) and codec format definitions | Big-endian 24-byte decoder config, S24, stereo left/right ordering and escape CPE syntax. ALAC escape frames are valid ALAC with explicit counts and END/padding, not PCM relabelled as compressed audio. Independent Symphonia decoding validates all 24 bits, channel order and sample rate. |
| [RFC 8439 section 2.8.2](https://www.rfc-editor.org/rfc/rfc8439#section-2.8.2) | Independent ChaCha20-Poly1305 known-answer test, including ciphertext and tag. Does not specify AirPlay. |

Wire layout: `[length BE16][marker/sequence BE32][sample timestamp BE32]
[0x15000000 BE32][encrypted ALAC][tag16][counter LE64]`.
The AEAD nonce is four zero bytes plus that counter; AAD is the eight bytes of
timestamp and format word. Key is the fresh media `shk`, independent of HAP
control-direction keys. Counter starts at zero, increases once per accepted
frame, does not track sequence wrap and fails closed before u64 exhaustion.
The maximum complete wire frame is 65535 bytes; empty, oversized, wrong-length,
reserved-bit and unsupported-format frames fail validation before decryption.

Sequence and marker are **outside AAD**. Tests explicitly demonstrate this
protocol limitation; they cannot be treated as authenticated identity or used
as nonce material. `open_frame` is an internal complete-frame diagnostic helper,
not a replay-protected receiving transport. No network receive buffer is added.
Marker scheduling, hardware format acceptance, optional SETUP fields, feedback,
latency and key acceptance remain source-derived assumptions requiring validation.
The offer deliberately does not impersonate `com.apple.Music`; whether firmware
requires additional client metadata is unresolved. RTSP 200 is not proof of audio.

The independent type103 byte-vector test was generated with Python
`cryptography.hazmat.primitives.ciphers.aead.ChaCha20Poly1305` (OpenSSL) and
`struct.pack`: key bytes 00..1f; counter `0x0102030405060708` packed `<Q`;
header packed `>III` from `0x807fffff,0x11223344,0x15000000`; payload hex
`2000123456789abc`; nonce `bytes(4)+counter`; AAD `header[4:]`; prefix
`struct.pack('>H',len(payload)+38)`. The payload is arbitrary crypto input,
not claimed to be a decodable ALAC fixture.

## Codec and capability decisions

| Format | Local encoder | Complete buffered session / hardware playback |
| --- | --- | --- |
| ALAC 48 kHz/24-bit stereo, ID21 | Implemented, fixed 352-sample escape frames; independent decoder tested | Disabled / unverified |
| AAC-LC 44.1 kHz stereo, ID22 | Unavailable | Disabled / unverified |
| AAC-LC 48 kHz stereo, ID23 | Unavailable | Disabled / unverified |
| AAC surround ID39, other ALAC rates/depths, unknown IDs | Unsupported | Disabled / unverified |

AAC-LC was evaluated first. [FDK AAC](https://github.com/mstorsjo/fdk-aac/blob/master/NOTICE)
has a custom license and patent conditions rather than a simple Apache/MIT grant;
it is not selected as a new native runtime dependency. FFmpeg's native AAC is a
possible future LGPL option, but adds a C toolchain/library and configuration-
dependent licensing surface ([FFmpeg legal](https://ffmpeg.org/legal.html)).
This PR chooses a small original ALAC encoder with no runtime codec dependency.
It does not assert that every AAC encoder is incompatible with Apache-2.0.
AAC remains prioritized for later bandwidth optimization after dependency review.
ALAC escape coding trades compression for low CPU and auditability: about
2.3 Mbit/s plus framing at 48 kHz stereo. No resampling, normalized-float capture
conversion, predictive compression, surround or realtime encoder reuse is hidden
inside this codec. Callers must supply correctly aligned 24-bit samples.

PR #39's model now distinguishes `FoundationOnly` from unsupported transports
and the realtime diagnostic implementation. ALAC24 encoder availability is true;
AAC is false, unknown IDs remain unknown. Authenticated format claims must be
valid and nonconflicting. Deterministic selection chooses ID21 when present,
regardless of extra unknown/unavailable formats; otherwise no format is selected.
`local_buffered_format` is capability advice, not a playback grant. Existing
allowlist, explicit authentication, endpoint and group restrictions still apply.
Advertisement, authenticated /info, local encoder and `RuntimeResults` remain
separate; no verified playback result is manufactured.

## Security, dependencies and validation

Existing RustCrypto `chacha20poly1305` 0.10 (Apache-2.0 OR MIT), `rand` OS entropy
and `zeroize` are reused. No new production dependencies or GPL code are added.
`socket2` 0.5 (MIT OR Apache-2.0), already in the lockfile transitively, becomes an
explicit dev dependency for safe send-buffer and reset fault injection. Existing
Symphonia ALAC/core 0.5.5 (MPL-2.0) remain test-only independent decoders. Original
ALAC code is Apache-2.0; no external codec source is incorporated.

No key, secret plist, plaintext sample or ciphertext is logged. Secret-bearing
owners have no Debug/Clone. Owned key/audio buffers are zeroized, but stack,
allocator, primitive-library and kernel copies are not guaranteed erased.
Fresh random keys prevent practical cross-session nonce/key reuse; entropy
failure is an error. This is not a cryptographic or hardware certification.

Tests cover independent decoder output, RFC and OpenSSL vectors, authentication
of timestamp/format/payload/tag/nonce, unauthenticated sequence limitation,
wrap/exhaustion, fresh sessions, one-shot offer, changed SETUP parameters,
malformed frames, local codec selection and authorization restrictions. A scripted
writer forces partial/Interrupted/WouldBlock writes; loopback receivers test
bounded backpressure, actual TCP stalls/deadlines, reset, orderly close and cleanup.
All automated sockets are loopback. No physical receivers are contacted.

## Historical PR #40 activation prerequisites

The following was the PR #40 prerequisite list. The [PR #41 timing update](PTP_TIMING.md)
implements timing/control components; the complete lifecycle is PR #42.

The future activation PR must provide the same-session authenticated engine, revalidate current
allowlist/selection and bind /info plus format selection to that session; create
the authorized data connection with bounded connect/cancellation; retain the
single Prepared key owner through negotiation; manage event/control sockets;
and tear down partial setup on every failure. It must implement actual PTP
clock identity, domain/peer policy, signaling, lock/holdover/discontinuity rules
and conversion to the 48 kHz sample timeline. `BufferedLifecycle`/`PtpAnchor` are
typed seams; PR #41 now supplies timing-control components but no complete lifecycle.

The engine must serialize the network seconds, full 64-bit fractional second
and timeline ID correctly (including signed plist bit-pattern requirements),
validate in-band anchor acceptance, arrange required SETPEERS and start anchors
before media delivery, and implement bounded prefill/latency/feedback, pause,
flush, resume, stop and authorization revocation. Restart requires new key and
nonce state. Group playback is separate work, never inferred from discovery.
Sanitized independent protocol/hardware evidence must resolve marker policy,
receiver format/key acceptance and teardown/feedback semantics before enabling
live playback. Audible output requires explicit physical verification on each
claimed receiver/firmware; mock success cannot substitute for it.

## PR #41 timing update

The [PTP timing foundation](PTP_TIMING.md) now supplies bounded packet handling,
a single-peer sender grandmaster, an independent offset/drift estimator, precise
48 kHz sample mapping and typed authenticated SETPEERS/SETRATEANCHORTIME requests
using this module's PtpAnchor boundary. Delay traffic is not receiver lock and
successful control is not a playback grant. Full BufferedLifecycle/session
ownership, feedback, media activation and hardware validation remain PR #42.
