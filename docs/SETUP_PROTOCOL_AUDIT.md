# Initial encrypted SETUP protocol audit (PR #23)

Base: fresh `origin/main` **ca74687e0898b303a8df44a4833b398b67345b96**,
including merged #22. Comparison: OwnTone **29.3**, tag peeled to
**d6fb3edf5831de38134ebd92fcf09a730ddd37aa**. This is a source audit and
synthetic regression exercise, not a hardware capture or receiver diagnosis.

## Finding

No independently verified runtime defect was found in the audited IPv4
pair-verify → encrypted GET /info → initial NTP SETUP path. Runtime behavior
is unchanged. In particular, the receive frame count shown in request logs
resets, but the cryptographic receive nonce does **not** reset. Successful
GET /info exercises both control keys, but cannot by itself establish the
correctness or receiver acceptance of the next request.

The highest-value next experiment is a passive, bidirectional capture of an
existing PiAir setup-only attempt, correlating TCP acknowledgements and any
UDP traffic to the advertised timing port during the SETUP wait. OwnTone's
NTP responder is already running when it sends SETUP; PiAir binds its timing
socket before SETUP but starts its responder only after the SETUP reply.
Whether this Apple TV waits for a timing exchange before replying is unknown.
Capture first; do not change timing behavior based on this hypothesis alone.

## Hardware observations (operator supplied; not reproduced here)

| Observation | What it establishes | What it does not establish |
| --- | --- | --- |
| AppleTV6,2, tvOS 26.6.1; persistent receiver identity; pair-verify succeeds | This run completes persistent verification | Acceptance of every later request |
| Encrypted GET /info: RTSP 200, 1672 response bytes, 2 authenticated frames | Receiver returned authenticated control traffic | A successful SETUP, or that counters after /info are correct |
| Initial NTP SETUP: 177-byte binary plist, CSeq 4; zero bytes after a verified 15-second timeout | No SETUP response bytes were read within that run's budget | TCP delivery/ACK, peer decryption, receiver reason, need for PTP |
| No RECORD or audio transmission | Failure precedes audio setup/use | Codec or audio incompatibility |

These observations supersede #21/#22's shorter timeout observations. No physical
Apple TV, HomePod or Raspberry Pi verification was performed for this audit.

## Source evidence table

Confidence refers to the source comparison; suspected impact remains unverified.
References P1–P4 and O1–O6 below identify the exact sources and functions.

| Component | PiAir behavior | OwnTone 29.3 behavior | Verified difference | Suspected impact | Confidence | Recommended next experiment |
| --- | --- | --- | --- | --- | --- | --- |
| Pair-verify secret/upgrade | X25519 shared secret; verifies pinned Ed25519 identity/signature, completes M4, installs control/event keys on same Wire/socket (P1) | HomeKit verify result supplies shared secret; `session_cipher_setup` installs cipher callback on same connection (O1,O2) | PiAir plaintext verify envelope uses HTTP/1.1 with Host/Connection; OwnTone uses RTSP/1.0 common envelope | Earlier envelope difference is tolerated in observed successful verification | High | Preserve pairing; correlate same TCP connection through SETUP |
| Control HKDF | SHA512, 32-byte keys, `Control-Salt`, Write for TX, Read for RX (P2) | Same labels and direction for channel 0 (O2) | None on this path | No key-label explanation found | High | Independent sequence oracle below; hardware trace if receiver authentication failure suspected |
| Nonce/framing | Four zero bytes + LE64 per-direction counter; LE16 length is AAD; ChaCha20-Poly1305, 16-byte tag; 1–1024 plaintext bytes (P2,P3) | Same layout on little-endian hosts; native-endian memcpy with TODO comments (O2) | PiAir explicitly specifies little endian; OwnTone code relies on host endian | No difference on Raspberry Pi little endian | High | Inspect actual outgoing frame lengths; do not infer frame count from CSeq |
| State across requests | ControlKeys allocated once at secure upgrade; independent TX/RX start at 0; each successful frame increments its own counter; no request reset (P2,P3) | Cipher context retained per session, independently incrementing counters (O1,O2) | None found | A hypothetical nonce reset would explain silence, but is contradicted by source and oracle | High | New fixture checks GET TX=0, GET RX=0,1, SETUP TX=1, SETUP RX=2 |
| Diagnostic counts | `response_bytes`/`response_frames` reset at request entry; include encrypted length/tag bytes; RX nonce lives elsewhere (P3) | Different logging/callback accounting | Metrics are not nonce counters | Avoid misreading zero SETUP frames as RX nonce zero | High | Correlate per-request logs with capture; never log keys |
| Partial writes | Encrypt a chunk once, retain ciphertext while `write_raw` advances an offset; no re-encryption on short writes; zero/error invalidates connection (P3) | evbuffer retains unwritten encrypted bytes and schedules further writes (O1,O3) | Blocking offset loop versus event buffering | No skipped/reused nonce on short writes found | High (source); forced short-write test absent | Backpressure/socket tracing only if capture shows incomplete outbound frame |
| Reads/boundaries | Read exact 2-byte length then ciphertext/tag; decrypt full frame, retain unread plaintext in 1024-byte pending buffer; headers/body can cross frames (P3) | Retain incomplete raw ciphertext; drain complete frames into plaintext evbuffer (O1–O3) | PiAir rejects surplus plaintext after one response; OwnTone buffers it | Strictness could reject actual extra data, but cannot explain zero received bytes | High | Fragmented oracle plus negative surplus fixture; capture any real surplus before broadening parser |
| Request sequencing | Blocking one request/response at a time; normal persistent path: verify CSeq 1/2, encrypted GET 3, SETUP 4 (P1,P3,P4) | Normal start: plaintext GET /info, verify, then encrypted SETUP; `next_seq` starts playback after verification (O1) | SETUP is normally OwnTone's first encrypted request, PiAir's second; both may have SETUP CSeq 4 | Possible receiver state-machine sensitivity; not evidence that encrypted GET is forbidden | High | If transport/timing trace is clean, separately test order in a future explicit diagnostic; no change here |
| SETUP request line/URI | `SETUP rtsp://<local IPv4>/<random u32> RTSP/1.0`; no port; GET uses `/info` (P3,P4) | Same local address/random unsigned session-id URI for IPv4 (O1,O3) | None for audited IPv4; OwnTone also formats bracketed IPv6 | No IPv4 URI defect identified | High | Verify local address and URI in sanitized trace |
| CSeq | Monotonic connection sequence including plaintext authentication; checked against reply (P3) | Monotonic session sequence starting at 1; deliberately tolerates response CSeq mismatch (O1) | PiAir response validation stricter | Could reject a received malformed reply, cannot explain silence | High | Authenticated wrong-CSeq regression; retain strictness |
| Content-Type / Content-Length | Nonempty SETUP gets `application/x-apple-binary-plist`; decimal serialized body byte length; GET length 0 (P3,P4) | Same SETUP content type; evrtsp adds decimal length for nonempty body, omits for empty GET (O1,O3) | Empty-request Content-Length present only in PiAir | GET already succeeds; no demonstrated impact on SETUP | High | Check full actual decrypted envelope only in a controlled private diagnostic |
| User-Agent | Persistent path `PiAir/0.1`; explicit transient profile uses `AirPlay/999.0.0` (P3) | Configured `general.user_agent`, default PACKAGE_NAME/PACKAGE_VERSION, normally `owntone/29.3` (O1,O6) | Different product string; deployed OwnTone configuration not inspected | Receiver filtering speculative | High for code, unknown deployed value | Compare accepted sender envelope before any single-header experiment |
| Client-Instance / DACP-ID | Same stable u64 rendered uppercase hex, no zero padding (P3) | Both uppercase libhash hex, no zero padding (O1) | Different actual sender identity, same format and persistence concept | No formatting defect found | High | Check consistency across verify upgrade, GET and SETUP; existing identity tests |
| Active-Remote | Persisted random u32, decimal (P3) | Low 32 bits of receiver `session->device_id`, decimal (O1) | Value provenance differs, type/width match | Likely remote-control routing; causal role unproven | High | Compare envelope first, do not replace identity speculatively |
| X-Apple-Client-Name | `PiAir` (P3) | Configured library name (O1) | Different value | No evidence name controls SETUP acceptance | High | Capture accepted sender value if available |
| X-Apple-ProtocolVersion | `1` on authenticated requests (P3) | Not added by audited airplay/evrtsp request path (O1,O3) | Present only in PiAir | Unknown-header sensitivity speculative | High | Single-variable experiment only after stronger transport/timing evidence |
| Other headers | No X-Apple-StreamID on initial SETUP; Session only after supplied by receiver (P3,P4) | StreamID compile-time flag disabled; no initial Session token in common builder (O1) | No material initial difference found; order differs | Header order has no demonstrated significance | High | Preserve current runtime envelope |
| NTP plist semantics | Exactly deviceID:string, sessionUUID:string, timingProtocol:string=`NTP`, timingPort:integer in 1..65535 (P4) | Same four fields; libplist string/uint constructors (O1,O4) | Insertion order differs (timingProtocol/Port); serializers may differ | Binary bytes/object order are not semantic requirements | High | Decode generated plist; compare typed values, not byte identity |
| UUID / sender deviceID | New v4 uppercase hyphenated UUID per negotiation; persistent sender u64 as eight uppercase colon-separated octets (P3,P4) | Random uppercase UUID (`uuid_make`); airplay_device_id=libhash, eight uppercase octets (O1,O5) | Concrete identity differs, representation matches; pairing controller ID is a separate identity | No requirement found that pairing UUID equal deviceID | High | Validate identity consistency and UUID format; no new identity mutation |
| NTP responder readiness | Timing socket bound before offer; responder starts after SETUP response validation (P3,P4) | Global timing service started during output init, before request; NTP offer references its port (O1) | PiAir cannot answer timing probes during the SETUP wait | Possible wait dependency; receiver behavior unknown | High source, low causal | **First priority: passive UDP/TCP trace during existing setup-only wait** |
| Response parsing / deadlines | RTSP/1.0 200, bounded headers/body, required CSeq, duplicate-header rejection; empty response may omit length. #22 opt-in uses one 15s post-write deadline with cancellation polling and retained offsets (P3) | evrtsp accepts RTSP/1.0/1.1, absent length means zero, different timeout/validation policy (O1,O3) | PiAir stricter; fixed diagnostic deadline is PiAir-specific | Parser cannot reject bytes never received; longer wait already failed on hardware | High | Keep strictness; capture peer close/ACK/probe behavior rather than extend timeout again |

## State-continuity audit details

`pairing::verify` moves the same Wire through `secure`; it does not reconstruct
the socket, sequence number, sender identity or pending buffer. Pair-verify
uses labeled `PV-Msg02`/`PV-Msg03` nonces with a separate derived key; those
messages do not consume control frame counters. Events keys are separate and
unused before initial SETUP. No reconnect, OPTIONS, RECORD or audio is inserted
between the normal persistent GET and SETUP.

`restart_deadline` only changes the deadline. `request` resets progress metrics
and increments CSeq through `authenticated_header`, but never constructs new
ControlKeys. An ordinary one-frame GET consumes TX nonce 0. A two-frame /info
reply consumes RX nonces 0 and 1, irrespective of CSeq and TCP segmentation.
The next one-frame SETUP must consume TX nonce 1, not 0, 2 or 4. Its first
reply must use RX nonce 2. The new independent fixtures exercise exactly this
asymmetric progression through the real Wire secure upgrade and request path.

Pending plaintext survives header/body reads within a response. Once the full
Content-Length body is consumed, request checks that no decrypted bytes remain.
Read errors, bad tags, malformed replies or residual plaintext close the
connection and remove keys; there is no retry with a consumed nonce. Short
writes continue with the same already encrypted frame. Interrupted I/O currently
fails closed rather than retrying; no evidence ties it to the observed run.
The #22 polling path retains read offsets across socket timeout polls and does
not restart its deadline when frames arrive.

## Regression evidence and limits

New generator: `tests/fixtures/generate_setup_sequence.py`, Python plistlib and
cryptography 50.0.0, fixed synthetic shared secret 00..1f and sender identity.
Fixtures are committed, deterministic and offline for cargo; the generator does
not import PiAir. They are independently computed protocol oracles, **not**
OwnTone output or proof of Apple TV acceptance. Request literals deliberately
record PiAir's current envelope; they do not declare those headers mandatory.

New tests cover:

- Secure upgrade followed by consecutive GET/SETUP; exact encrypted outgoing
  envelope, CSeq, URI, headers and Content-Length against Python-generated bytes.
- Two-frame binary /info response with fragmented network writes, followed by
  an empty SETUP response without Content-Length; checks no stale plaintext
  contaminates the second request and no diagnostic reset resets nonces.
- Authenticated wrong CSeq, duplicate Content-Length, surplus plaintext and
  correctly tagged data at a reset RX nonce all fail closed.
- Outbound 2304-byte message matches an independent three-frame oracle,
  including maximum-size blocks, a partial final block and all byte values.
- Decoding the generated NTP offer through the general plist reader, exact four
  typed fields, full-width sender ID, uppercase UUID and ports 1/7010/65535;
  semantic equality with Python's independent plist, without byte equality.

Existing tests additionally exercise actual persistent pair-verify followed by
GET/SETUP/TEARDOWN, UUID v4 generation and sender/header consistency, independent
single-SETUP ciphertext and directional crypto fixtures, bad tags, replay,
length limits, counter exhaustion, delayed/partial reads, fixed deadlines,
cancellation and no RECORD/audio for setup-only checks.

TCP may coalesce the seven-byte fixture writes; they are not a guarantee of
seven-byte kernel reads. Exact frame reads and split headers/body are exercised,
but forced kernel short writes and real packet loss/backpressure are not.
No transport abstraction or broad refactor was added solely to inject these.
The oracle supplies the completed pair-verify secret; existing signature/verify
mock tests cover that preceding cryptographic handshake. No live OwnTone or
autostream service was contacted, modified or restarted.

## Next diagnostic decision

Use a private passive trace of the already authorized setup-only attempt and
its existing sanitized logs. Record commit/model/OS, connection tuple, SETUP
CSeq, body length, outgoing encrypted frame length, TCP ACK/close/reset, timing
UDP destination/source and relative timestamps through the 15-second window.
Do not publish pairing traffic, private headers, decrypted payloads or keys.
No new Raspberry Pi deployment commands are supplied because this PR introduces
no runtime correction.

If the complete SETUP frame is ACKed and timing probes arrive at the advertised
port with no PiAir answer, that makes responder readiness the first targeted
follow-up experiment; it still does not prove the receiver gates its reply on
NTP. If no timing probes arrive, compare a known accepted sender's initial
control trace (if already available) before testing one sequencing/header
variable at a time in a separate diagnostic. Do not alter/restart OwnTone to
obtain that comparison under this task. TCP ACK establishes kernel delivery,
not successful application authentication. Zero response bytes alone cannot
distinguish receiver decryption failure, policy rejection or a wait dependency.

PTP, FairPlay, codecs, audio changes and speculative protocol variants remain
outside this PR. OwnTone's explicit NTP implementation prevents interpreting
advertised PTP or continued silence as evidence that PTP is required.

## Source index (immutable OwnTone 29.3 links)

- P1: `src/receiver/pairing.rs`: `verify`, `verify_inner`.
- P2: `src/receiver/crypto.rs`: `derive`, `ControlKeys::{new,encrypt,decrypt}`.
- P3: `src/receiver/wire.rs`: `secure`, `authenticated_header`, `request`,
  `read_raw`, `read_exact`, `write_raw`, `write_message`, `response`,
  `sender_device_id`, `bind_media`, `start_timing`.
- P4: `src/receiver/session.rs`: `call`, `negotiate`; `profile.rs`: `ntp_offer`.
- [O1: airplay.c](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c):
  `request_headers_add` (889), `rtsp_cipher` (1005), `session_pair_success`
  (1346), `session_cipher_setup` (1438), `session_ids_set` (around 1490),
  `payload_make_setup_session_ntp` (2691), `response_handler_info_start`
  (3373), `response_handler_pair_verify2` (3548), sequence table (3640),
  `sequence_continue` (3774), output init/timing service (4293–4314).
- [O2: pair_homekit.c](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/pair_ap/pair_homekit.c):
  key-label map (106), `hkdf_extract_expand` (882), verify implementation
  (around 1700–1930), `cipher_new` (2889), `encrypt`/`decrypt` (2945/3001).
  [pair.c](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/pair_ap/pair.c)
  dispatch/rollback (689–749) also inspected.
- [O3: evrtsp/rtsp.c](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/evrtsp/rtsp.c):
  `evrtsp_make_header_request` (326), `evrtsp_write` (455), input cipher
  callback (638–646), output callback (745–751), response parsing (896,1146).
- [O4: plist_wrap.h](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/plist_wrap.h):
  `wplist_dict_add_string`, `wplist_dict_add_uint`, `wplist_to_bin`.
- [O5: misc.c](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/misc.c#L2066): `uuid_make` (both compile-time branches).
- [O6: conffile.c](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/conffile.c#L76): default user_agent.

## Local validation

Environment: x86_64 Linux, Rust 1.99.1; official Debian libasound2-dev
1.2.14-1+deb13u1 extracted locally, using installed ALSA runtime. Toolchain,
headers and library search paths were configured outside the repository.

| Check | Result |
| --- | --- |
| `cargo fmt --check` | Pass |
| `cargo clippy --all-targets -- -D warnings` | Pass |
| `cargo test --locked` | Pass: 143 unit tests and 13 CLI tests |
| `cargo build --release --locked` | Pass |
| `git diff --check` | Pass |
| Regenerate all 11 sequence fixtures and compare SHA256 hashes | Identical |

The initial sandboxed test run failed socket-based tests because loopback socket
operations were denied (72 passed, 71 failed). The complete exact command was
then rerun with loopback permitted and passed, including all new tests. No tests
were skipped or relaxed. No Raspberry Pi build or hardware success is claimed.
