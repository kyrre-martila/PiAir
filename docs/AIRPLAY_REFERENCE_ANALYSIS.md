# Sender source analysis and implementation decision (Prompt 4c)

Examined 2026-11-07, before adding the active transport. GitHub reported PR #4
already merged at `4fa1401` when this continuation began; the user explicitly
authorized a follow-up PR based on that main, preserving the completed work. This is a protocol fact
ledger, not a port of implementation source. OwnTone Mini 1.5.0 is a local
product version; no correspondence with an upstream version is assumed. No Pi,
SSH, installed services or Autostream scripts are accessed.

## Pinned sources and licenses

| Repository | Examined revision | License / use |
| --- | --- | --- |
| owntone/owntone-server | f7ace58dc2aeb5305360770da37fd62095f5bb21 | Main sender GPL-2.0-or-later; inspect protocol behavior only, no code reuse/linking. Bundled libairptp has a separate MIT license but is not imported. |
| postlund/pyatv | b277a4c8222ecdcbaab8a24e3e713ca44765adb4 | MIT; independent corroboration of actual UDP audio sender, event channel, NTP and feedback. No implementation code incorporated. |
| openairplay/airplay2-receiver | 6c343d3679ddb561c61566985acaaf587d0a3bd3 | No repository-wide license grant found; protocol/receiver-parser observations only. |
| philippe44/RAOP-Player | c14088da44df37f58ef7a45b8ce8a3cbc051c5e4 | Root MIT with third-party exceptions; inspect only. Classic RAOP timing is not proof of AirPlay 2 authentication. |
| openairplay/airplay2-protocol | unavailable | Public clone returned Repository not found. No evidence attributed to it. |

Source links below are relative to
[the pinned OwnTone tree](https://github.com/owntone/owntone-server/tree/f7ace58dc2aeb5305360770da37fd62095f5bb21)
and [the pinned pyatv tree](https://github.com/postlund/pyatv/tree/b277a4c8222ecdcbaab8a24e3e713ca44765adb4).

## Crucial result: OwnTone's AirPlay 2 sender is realtime UDP

`src/outputs/airplay.c`: `AIRPLAY_RTP_PAYLOADTYPE=0x60` (96),
`AIRPLAY_SAMPLES_PER_PACKET=352`, default 44100/16/2. Buffered 103 appears in
comments, not as the active transport. `response_handler_setup_stream` connects
`SOCK_DGRAM`; `packet_send` sends datagrams. There is no active type-103 TCP
sender or SETRATEANCHORTIME flow in this file. Do not describe it as buffered
just because it uses HAP, two SETUPs, plists or an output buffer.

PiAir therefore selects **AirPlay 2 persistent-HAP, NTP, realtime UDP, ALAC
44100/16/stereo, 352 samples**. This is the verified OwnTone variant selected
when PTP is disabled, corroborated by pyatv's NTP AirPlayV2 sender. It is not a
legacy ANNOUNCE/RSA/AES RAOP session. No automatic PTP/NTP fallback is proposed.
The earlier 48 kHz/type-103 components remain separate, disabled components.
No claim that every modern Apple TV accepts this profile is possible without
hardware validation. PTP-only or FairPlay/MFi-required receivers must fail.

## OwnTone call trace: control to actual encoded bytes

All names in this table are source references, not PiAir architecture choices.

| Operation | File and function | Observable behavior |
| --- | --- | --- |
| Discovery/selection | `src/outputs/airplay.c`, `airplay_device_cb`, `features_parse` | AirPlay TXT deviceid/features; requires audio and CoreUtils pairing/encryption; sets PTP according to capability/config; fixes ALAC 44100/16/2. Model classification affects keepalive/reconnect, not codec discovery. |
| Start/control socket | same, `airplay_device_start`, `session_make`, `session_connection_setup`, `sequence_start` | Creates RTSP control TCP; GET /info selects authentication from statusFlags and stored credentials. |
| Persistent/transient authentication | same, `response_handler_info_generic`, `payload_make_pair_generic`, `response_handler_pair_verify2`; `src/pair_ap/pair_homekit.c` | Persistent credentials use pair-verify; transient can be tried and 470 leads to PIN setup. PiAir keeps its explicit persistent method and signed receiver verification; no automatic transient fallback or credential deletion. |
| Control protection | `session_cipher_setup`, `rtsp_cipher`; `pair_homekit.c`, `cipher_new`, `encrypt`, `decrypt` | Fresh verify shared secret; Control-Salt directional keys; LE length/AAD, ChaCha20-Poly1305 and directional counters. Existing PiAir already implements this. |
| Session identifiers | `session_ids_set`, `request_headers_add` | Fresh UUID/session numeric ID; URI uses sender address/session ID; CSeq advances. No need to pretend PiAir is iOS or to run a DACP service. |
| Session SETUP | `payload_make_setup_session_ntp` | Binary plist deviceID/sessionUUID/timingPort/timingProtocol=NTP. PTP branch separately advertises timingPeerInfo/list, clock identity and group. None timing is explicitly reported not to work with ATV4. |
| Event connection | `response_handler_setup_session`; `src/outputs/airplay_events.c`, `client_add`, `incoming_cb`; `pair_homekit.c` channel salts | eventPort; TCP to the same receiver; Events-Salt with directions reversed from control perspective; encrypted receiver requests get correlated RTSP responses. OwnTone tolerates event connect failure; PiAir should require it. |
| Actual startup sequence | `airplay_seq_request[AIRPLAY_SEQ_START_PLAYBACK]` | SETUP session → RECORD → SETPEERS (PTP only) → SETUP stream → SET_PARAMETER volume. No buffered rate anchor. |
| Stream offer | `payload_make_setup_stream` | audioFormat=0x40000, ct=2, spf=352, sr=44100, type=96, latencyMin=11025/Max=88200, controlPort, isMedia, shk, streamConnectionID, supportsDynamicStreamID=false. |
| Ports | `response_handler_setup_stream` | Parse stream dataPort/controlPort and connect UDP audio. PiAir must validate type/ranges, restrict every address to the authenticated peer, and recheck approval. |
| Audio source/encoder | `airplay_write`, `packets_send`, `alac_encode`, `master_session_make`; `src/transcode.c` | Raw player PCM is accumulated into 352 samples; FFmpeg ALAC output is put into RTP payload. This is actual audio transmission, not remote-control-only support. PiAir can use its independently decoder-validated ALAC escape frames with 44100 configuration. |
| Media key/encryption | `session_cipher_setup`, `payload_make_setup_stream`, `packet_encrypt` | shk is a 32-byte media key carried inside encrypted SETUP. OwnTone reuses the verify-secret bytes; pyatv uses different derived bytes, corroborating receiver consumption of the offered key. PiAir can generate a fresh independent random media key. Header timestamp/SSRC is AAD; ciphertext/tag/8-byte nonce. |
| Nonce limitation | `packet_encrypt` | OwnTone uses native u16 sequence as nonce material, which repeats after wrap under the same key. PiAir must not reproduce that unsafe choice; pyatv uses a monotonic independent nonce counter and the receiver consumes the transmitted nonce. Retransmit cached ciphertext unchanged. |
| RTP/send/cache | `packets_send`, `packet_send`; `src/outputs/rtp_common.c`, `rtp_packet_next`, `rtp_packet_commit` | Normal 12-byte RTP, PT96, first-packet marker, 16-bit seq, 32-bit sample timestamp, SSRC; datagrams; bounded resend history. |
| NTP clock | `timing_svc_cb`, `timing_get_clock_ntp`; `rtp_common.c`, `sync_packet_ntp_make` | 32-byte d2/d3 query/reply with origin/receive/transmit stamps; 20-byte d4 sync maps rendering RTP position and next packet to NTP. Receiver follows sender clock; no buffered rate anchor is needed. |
| PTP alternative | `payload_make_setup_session_ptp`, `handle_timingpeerinfo`, `packets_sync_send`; `src/libairptp/src/airptp.c`, `ptp_msg_handle.c` | Separate clock negotiation and d7 sync with clock ID, backed by libairptp. Not mixed into the chosen NTP profile. |
| Latency/pacing | `master_session_make`, `timestamp_set`, `packets_sync_send` | Player sample clock, output-buffer offset, min latency; sync about once per second. PiAir's deterministic source must use absolute sample deadlines and monotonic-to-NTP mapping, not arbitrary sleeps or unbounded prefill. |
| Retransmission | `control_svc_cb`, `packets_resend` | 8-byte 0x80d5 request gives first u16 seq/count. OwnTone resends original media over data UDP; distinguishes this from a documented alternative d6 control reply. |
| Liveness | `airplay_keep_alive_timer_cb`, `airplay_metadata_keep_alive_send`, feedback sequence | Keepalive/progress workaround every 25s for some ATV/HomePod versions; POST /feedback exists. pyatv uses 2-second feedback. Short PiAir diagnostic needs bounded feedback and timing stalls, not resurrection loops. |
| FLUSH/TEARDOWN | `payload_make_flush`, `payload_make_teardown`, response handlers, `session_failure`, `session_cleanup`, `session_free` | FLUSH carries next seq/rtptime; TEARDOWN empty plist closes session; cleanup releases transports/ciphers. PiAir must clean partial setups without copying OwnTone's event-loop structures. |

## pyatv comparison and independent corroboration

`pyatv/protocols/raop/protocols/airplayv2.py`, `AirPlayV2._setup_base`,
`setup_audio_stream`, `send_audio_packet`, `_feedback_task_loop`: actual
AirPlay 2 UDP audio, HAP verification, encrypted event channel and NTP. Its audio
offer is **PCM** ct=1/audioFormat=0x800, not OwnTone's ALAC offer. It supplies
shk independently of the raw HAP secret, uses timestamp/SSRC AAD and a separate
incrementing nonce. These corroborate channel/key/packet mechanisms, not ALAC
format identifiers. PiAir selects OwnTone's ALAC tuple rather than mixing PCM
bytes into ALAC or using the old buffered packet header.

`pyatv/protocols/raop/stream_client.py`, `StreamClient.initialize`, `send_audio`,
`_stream_data`, `_send_packet`, `ControlClient._sync_task`; `protocols/__init__.py`,
`TimingServer`: UDP packet pacing, initial RECORD/FLUSH, NTP reply and sync,
retransmit handling, latency padding. Sequence ordering differs: pyatv's stream
SETUP precedes RECORD; PiAir follows OwnTone's complete ordering, not a blend.

`pyatv/protocols/airplay/ap2_session.py` and `channels.py` also implement
remote-control-only sessions/type130. `player.py` supports URL playback. Those
are distinct from the actual UDP audio path above. `auth/hap.py` and
`auth/hap_transient.py` implement persistent/transient HAP; `auth/legacy.py` is
another family. PiAir retains persistent signed identity enrollment/verification.

RAOP-Player's `src/raop_client.c` is a classic RAOP sender, useful for timing/RTP
corroboration, not evidence for HAP/plist negotiation. The pinned openairplay
receiver corroborates offered shk and normal realtime RTP/tag/nonce layout;
its buffered 24-bit sequence class belongs to another transport entirely.

## Originality and concrete implementation limits

No GPL source is vendored, linked, translated, or used as an implementation
skeleton. This single-agent investigation is source-informed independent
implementation, not a falsely claimed personnel-separated clean room. Protocol
fields/constants and wire semantics are recorded above; PiAir retains its own
linear ownership/authorization model, standard Rust crypto, bounded parsers,
clock mathematics and tests. There is no evidence that this requires copying
protected expression or an incompatible dependency. Preserve Apache-2.0.

Hardware-dependent items: acceptance of NTP on a particular tvOS policy; precise
latency response behavior; receiver event variants; actual audible playback.
Unsupported: buffered103/PTP-only variants, transient/MFi/FairPlay/password/Home
membership paths. Such requirements fail explicitly. Acknowledged negotiation
and transmitted packet counts do not prove physical playback.

## Implemented result and validated bounds

The selected NTP/realtime profile is now active only through interactive
`stream-test`. Production code authenticates, negotiates both SETUPs and RECORD,
services encrypted events/NTP, encodes and encrypts ALAC, sends UDP media and sync,
handles bounded retransmission/feedback, and acknowledges FLUSH/TEARDOWN. The
same implementation passes loopback full-path tests with independently decoded
audio and synthetic plist/AEAD/event-key fixtures. No hardware playback is
claimed. Current exact implementation, limits and state transitions are in
AIRPLAY_SESSION.md.

Concrete differences/limits remain explicit: require an authenticated positive
NTP timingPort (OwnTone accepts timing queries more permissively); require an
initial encrypted updateInfo and NTP exchange before READY; refuse zero/out-of-
window Audio-Latency; do not implement OwnTone's PTP/libairptp branch, automatic
transient fallback, password digest, metadata resurrection or long sessions.
These are deliberate bounded profile choices, not missing undocumented constants.
For a receiver requiring PTP, the unimplemented work is the exact PTP session
SETUP/SETPEERS, `handle_timingpeerinfo`/libairptp clock service and d7 sync already
identified above. For buffered type103, the pinned OwnTone file has no active
sender path to trace; the old buffered components are not promoted to playback.
