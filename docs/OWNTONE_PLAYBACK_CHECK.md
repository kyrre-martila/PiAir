# Receiver stream negotiation and bounded playback diagnostics

These modes use an allowlisted compatible receiver selected by canonical ID or
the existing interactive picker. See [receiver selection](RECEIVER_DIAGNOSTICS.md).
No receiver was contacted during development. Do not merge automatically.

## Evidence and source comparison

Fresh `origin/main` was `6be48a9b2db50b5ccffb32ee413e164954f0baa5`.
GitHub reports PR #35 merged at 2026-11-08 20:36:03 UTC; that merge and its
RECORD implementation are present locally. The operator-supplied physical PR #35
evidence establishes transient SRP, encrypted GET /info, initial NTP SETUP,
event TCP connection, RECORD 200, three NTP replies, TEARDOWN 200 and local
resource release. No audio was transmitted. This supersedes the historical
“RECORD acceptance unverified” statement in the PR #35 document; we did not
repeat that physical test.

Protocol analysis uses only OwnTone commit
[`f7ace58dc2aeb5305360770da37fd62095f5bb21`](https://github.com/owntone/owntone-server/tree/f7ace58dc2aeb5305360770da37fd62095f5bb21).
Source was inspected before implementation, including `src/outputs/airplay.c`,
`rtp_common.c/.h`, event channel implementation, and ALAC encoding setup.
PiAir's session, profile, wire, realtime, codec, CLI, fixtures and earlier
OwnTone comparisons were reviewed. Protocol behavior is independently
implemented; GPL implementation code is not copied.

| Topic | Verified pinned OwnTone fact | PiAir decision |
| --- | --- | --- |
| Sequence | `airplay.c` sequence table near L3640: initial SETUP, RECORD, SETPEERS, stream SETUP, volume. `payload_make_setpeers` L2660 skips NTP sessions. | Preserve PR #35 through RECORD; immediately send stream SETUP. No SETPEERS. |
| Stream plist | `payload_make_setup_stream` L2613: root dictionary, `streams` array of one dictionary. | Same exact 13 fields below, binary plist. |
| Stream response | `response_handler_setup_stream` near L3040: first stream dictionary with nonzero dataPort/controlPort; no mandatory echoed type. | Exactly one stream, typed nonzero u16 distinct UDP ports, no overlap with event port; optional type must be 96. Optional codec/latency/encryption fields must match offer; optional connection ID must match. Unsupported timing, media policy or audio mode fails. Unknown fields do not authorize new behavior. |
| Key | `session_cipher_setup` L1438 and stream offer L2630: first 32 bytes of verified pairing shared secret; transient secret has 64 bytes. | Fix the approved OwnTone path to use SRP K[0..32], with no additional audio HKDF. Other profiles keep their existing behavior. Control and event keys still use the established HKDF labels and independent directional counters. Audio key is zeroizing and removed from Wire when consumed. |
| Codec | OwnTone ALAC encoder setup near L1152: 44,100 Hz, signed 16-bit stereo; 352 frames per RTP packet. | Reuse existing original ALAC escape-frame encoder, explicit frame count, CPE, left/right S16 bit packing, END and padding. Escape coding is lossless ALAC; it need not match ffmpeg compression byte for byte. Independent Symphonia decoding tests cover 44.1 kHz. |
| Audio UDP | `rtp_common.c` L148–185 and `packet_encrypt` L1916: 12-byte RTP header, payload type 96, network-order u16 sequence and u32 sample timestamp/SSRC; encrypt ALAC only; AAD header bytes 4–11; ciphertext, 16-byte tag, 8-byte nonce suffix. No TCP prefix. | Reuse existing Encoder/MediaIo; first packet marker gives byte 0xe0, subsequent 0x60 (`packets_send` adds the marker when the session first connects). Sequence +1 modulo 65536, timestamp +352 modulo 2^32. Original key + counter stay in one encoder. |
| Nonce | OwnTone copies native u16 sequence into bytes 4–5 of a zeroed 12-byte nonce and transmits the final 8 bytes; little endian on Pi/x86. Retransmissions reuse original ciphertext. | Start the little-endian counter at initial RTP sequence; choose a sequence low enough that the fixed 376-packet budget cannot wrap. This matches OwnTone on little-endian Pi/x86 without reproducing its nonce repetition after wrap. No packet retry in the diagnostic; a retransmission request fails closed. |
| NTP and sync | `timing_svc_cb` L2262 answers 32-byte 80d2 requests with 80d3. `packets_sync_send` L2161 and `sync_packet_ntp_make` in rtp_common.c use initial 90d40007, later 80d40007, current sample position/NTP/next RTP timestamp. | Retain continuous authorized PR #35 responder before initial SETUP through TEARDOWN. Playback requires at least one valid answered timing query within a bounded 3-second readiness wait and healthy event/NTP workers. A reply proves local timing activity, not measured receiver clock convergence. Stream-only does not require a timing query. |
| Latency and pacing | `airplay.c` L1207 uses output buffer = start buffer minus 250ms; offered latencyMin=11025, latencyMax=88200. | Existing 2-second total lead: sync position = next timestamp −77175; NTP maps to an absolute monotonic sample deadline. Sync every ~1 second. Pace 352/44100 seconds, fail if >100ms behind; no uncontrolled catch-up. RECORD Audio-Latency remains informational. |
| Feedback | `AIRPLAY_KEEP_ALIVE_INTERVAL` L113 is 25 seconds; feedback sequence near L3701. Event channel need not receive an unsolicited event before continuing. | Three-second diagnostic finishes before keepalive is due; no speculative feedback. Retain event decryption/ACK/health checks and continuous NTP. Normal playback keeps existing feedback behavior. |
| Shutdown | OwnTone stop sequence uses TEARDOWN; empty plist builder near L2423. | Diagnostic sends empty binary dictionary TEARDOWN, without FLUSH, on the same encrypted control session. Require 200, matching CSeq and empty response. Failed/invalidated/revoked transport cannot be called acknowledged. Bounded failure cleanup retains existing 250ms request budget. Close sockets, join workers and zeroize keys on every exit. |

### Exact stream offer

Root `streams: array[dictionary]`, with one entry:

| Field | Plist type | Value |
| --- | --- | --- |
| audioFormat | integer | 262144 (0x40000) |
| audioMode | string | default |
| controlPort | integer | locally bound nonzero u16 UDP control port |
| ct | integer | 2 (ALAC) |
| isMedia | boolean | true |
| latencyMax | integer | 88200 samples |
| latencyMin | integer | 11025 samples |
| shk | data | 32 bytes, verified SRP K prefix; never logged |
| spf | integer | 352 |
| sr | integer | 44100 |
| type | integer | 96 (realtime UDP) |
| supportsDynamicStreamID | boolean | false |
| streamConnectionID | integer | same random u32 session number used in RTSP URI |

CSeq remains uninterrupted: plaintext GET /info 1, M1 2, M3 3, encrypted
GET /info 4, initial SETUP 5, RECORD 6, stream SETUP 7. Stream-only TEARDOWN
is 8. Playback volume is 8, then TEARDOWN 9. No control CSeq is consumed by
UDP timing, sync, audio or event ACKs. Earlier failure changes the final CSeq.
Common headers, Session token, RTSP URI, framing and independent control cipher
counters remain as documented in [PR #35](OWNTONE_RECORD_CHECK.md).

## Fixed limits and authorization

Both flags require `--experimental-owntone-transient`,
existing allowlist approval, explicitly selected transient method, current mDNS
identity/capability/endpoint agreement, pinned control peer and interactive
y/yes confirmation. Omit the receiver ID to use the existing picker. Neither mode permits another diagnostic,
`--seconds`, `--volume-db`, setup profile or setup timeout override. Unsupported
selected capabilities fail before control contact; encrypted capabilities are
validated before SETUP. No pairing, enrollment, credential deletion, receiver
fallback or automatic retry occurs. Transient SRP verifies a session proof,
not persistent receiver identity. Existing metadata/endpoint pins are necessary.

`--stream-setup-check` stops after strict stream response validation. Zero audio,
zero sync datagrams, zero SET_PARAMETER and zero volume changes. Successful
stream SETUP does not prove audible playback.

`--playback-check` sets receiver volume to **−40 dB**, requires acknowledgement,
then timing readiness. Volume may persist even if synchronization or playback
fails. Restoration is not implemented or claimed. It transmits exactly **376**
audio packets on success: 132352 encoded sample frames, covering 44100 tone
frames and silent drain/padding. The original 441 Hz triangle tone peaks at
4100/32768 (about −18 dBFS), with 50ms attack/release. Tone content is limited
to one second. Media pacing/drain is three seconds; final packet padding is
silent. Transmission is bounded by fixed sample count and scheduler deadlines;
a cancellation/failure may transmit fewer packets. No retransmissions. Receiver
acoustics and volume scaling are unverified: the operator must be ready to stop.
Successful packet transmission does not prove the HomePod actually played sound.

Cancellation, changed approval/selection, invalid response, worker failure,
transport error or timeout fails closed. Revocation prohibits further sends,
including TEARDOWN. A skipped/failed remote cleanup is reported separately from
local release; remote resource expiry remains uncertain. No production OwnTone,
autostream, systemd services, other receivers or deployed binaries are modified.

Authorization polling previously called the full logging config loader for every
packet/poll. It now rereads the file at each boundary and caches successful
parsing only for exactly identical bytes, separately per thread. A changed file,
atomic replacement, malformed TOML or read failure is evaluated immediately;
there is no mtime/TTL cache. Selection is still reloaded and compared every time.
Failed checks remain errors. Deterministic replacement/revocation tests cover it.

## Receiver selection and operator usage

See [receiver discovery, selection and diagnostics](RECEIVER_DIAGNOSTICS.md)
for current commands, canonical ID placeholders, picker behavior and confirmations.
Use an independently verified allowlisted compatible receiver. No physical
receiver contact or Raspberry Pi service changes were performed for this change.

## Validation and remaining uncertainty

Loopback fixtures use real SRP, continuous independent control/event cipher
state, exact request/CSeq/header/plist assertions, key equality to verified SRP,
receiver-side ChaCha20-Poly1305 verification, independent ALAC decoding, packet
sequence/timestamp/nonce progression, sync framing, bounded duration/pacing,
zero audio/volume in stream-only, credential byte preservation and timing port
rebinding after cleanup. Failure cases and normal playback/PR #33–35 regressions
remain required checks. Structured logs include phase/outcome, negotiated codec
and ports, volume acknowledgement, RTP/NTP counts, elapsed playback, TEARDOWN
and cleanup, never keys or decrypted payload dumps.

Source observations and loopback tests cannot establish physical stream SETUP
acceptance, the HomePod's actual timing convergence, 
volume-to-acoustic mapping, audible playback, ARM64 execution or resource expiry
after failed remote cleanup. These remain explicit physical-test uncertainties.

Final validation on Rust 1.99.1 / x86_64 Linux:

- `cargo fmt --check`: pass.
- `cargo clippy --all-targets --locked -- -D warnings`: pass.
- `RUST_TEST_THREADS=2 cargo test --locked`: 190 unit tests and 18 CLI tests pass.
- `cargo build --release --locked`: pass.
- `git diff --check`: pass.

Rust tooling and official Debian ALSA development headers were prepared under
workspace tooling only. Dependencies and lockfile are unchanged. These local
checks used loopback fixtures; no physical receiver test was performed.
