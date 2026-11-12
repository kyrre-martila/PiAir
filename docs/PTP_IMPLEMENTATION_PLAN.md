# PTP realtime negotiation (PR 62)

## Reference and scope

Behavioral reference: [OwnTone 8ed84f2b3cba25d3de65e05660d59ce034b5adab](https://github.com/owntone/owntone-server/tree/8ed84f2b3cba25d3de65e05660d59ce034b5adab).
The seven requested files were inspected before implementing the changes.
PiAir's Rust implementation is independent Apache-2.0 code. No OwnTone code was
copied, translated, linked, or vendored. Wire constants/layouts and observable
control order are protocol facts; private Apple fields are identified as opaque.

This supersedes the earlier plan's permanently closed *realtime* readiness gate.
The legacy `buffered-test` command and its type-103 anchor experiment remain
separate. The selected profile here is **type 96, UDP/RTP, ALAC16 stereo,
44,100 Hz, 352 samples/packet, PTP RTCP synchronization**. PR 62 sends no RTP
media, RTCP announcements, PCM, volume request, or WAV data. PR 63 connects the
existing WAV/RTP scheduler to the owned negotiated session.

## Compatibility matrix

All OwnTone names below refer to the immutable reference above. Hardware results
are distinguished from source observations and regression models.

| Area / reference | Earlier PiAir difference | Resolution in PR 62 |
| --- | --- | --- |
| `airplay.c`: `airplay_seq_request`, START_PLAYBACK | Diagnostic ended after SETPEERS; buffered progression added unrelated controls | Realtime sequence is initial SETUP → RECORD → SETPEERS → stream SETUP. No command registration, audioMode POST or SETRATEANCHORTIME. |
| `airplay.c`: `payload_make_setup_stream` | PTP lifecycle used buffered ALAC24/48k/type103/TCP | Reuse `profile::stream_offer`, existing ALAC16 `realtime::Encoder`, type96 UDP sockets. No second codec/media engine. |
| `airplay.c`: `response_handler_setup_stream` | No realtime stream negotiated in the PTP owner | Require one stream and nonzero distinct dataPort/controlPort; validate optional type, codec/rate/SPF, connection ID, policy and PTP echoes; preserve RTSP cipher/CSeq/session binding. |
| `airplay.c`: `/info` and PTP initial SETUP | Buffered feature bit40 and format21 were prerequisites | Realtime requires audio/ALAC/PTP bits9/19/41 and compatible audioStream format when present. Same authenticated deviceID and access-policy checks, including verified StoredHap exception for bit9. |
| `airplay.c`: timingPeerInfo/list, `handle_timingpeerinfo` | Initial PTP infrastructure already implemented | Retain signed ClockID, local addresses, receiver-address validation, selected authenticated peer, encrypted event owner and no fallback. |
| `airplay.c`: volume is last startup step | Volume is outside this PR's authorization | Explicitly omit SET_PARAMETER volume. Reference supplies no required realtime anchor/audioMode/command step in its startup table. This is a deliberate product constraint, requiring Apple TV verification. |
| `ptpd.c`: clock/peer delegation | PiAir is a single-session CLI, not a shared daemon | One session owns identity, epoch, sockets, authorization lease and cleanup. No shared daemon discovery or peers learned from UDP. |
| `daemon.c`, `airptp.c`: bound services, peer registration, periodic timers | Timing polls could run only between blocking control requests | Existing scoped timing worker is retained throughout every bounded authenticated request and healthy TEARDOWN; joins before advancing/dropping. Observe loop also polls. |
| `ptp_msg_handle.c`: header_init/dispatch | Per-message-role port pin rejected a later legitimate identity | Authorize source ClockID and connected selected-IP/channel endpoint. Track sequence history per full sourcePortIdentity **and kind**; accept concurrent port numbers in one role. No clock replacement, BMCA or servo. |
| `ptp_msg_handle.c`: Sync/Follow_Up | One incoming pending pair globally | Pending two-step pairs are per source port; exact sequence/port correlation, expiry and duplicate rejection. Outgoing Follow_Up follows actual local Sync socket send using the same monotonic epoch. |
| `ptp_definitions.h`, `ptp_msg_handle.c`: layouts/flags/intervals | Missing PTP timescale bit and response intervals | Sync 0x0608/-3; Follow_Up 0x0408/-3; Announce 0x0408/0; Signaling 0x0408/-128; Delay_Resp 0x0608/-3. IEEE control fields emitted; known reference legacy-zero controls accepted for applicable messages. |
| `ptp_msg_handle.c`: `msg_signaling_make`, `daemon.c` timers | Received Signaling was only an inert observation; none sent | Emit Signaling immediately and every second with zero target and Apple OUI 00:0d:93 subtypes1/5, lengths22/32 and opaque prefix 00 00 03 01. Independent wrapping sequence. |
| `ptp_msg_handle.c`: organization handlers | Earlier documentation implied Signaling should mutate peers | Reference interval handler logs/ignores intervals; Apple private handlers do not authenticate peers. Own OUI peer-add/del messages are **local daemon IPC**, not receiver authority. PiAir never implements network-driven peer add/delete, redirection or grants. |
| `ptp_msg_handle.c`: Follow_Up information extensions | Private clock TLV forced equality with source ClockID | Keep bounded IEEE Follow_Up info and Apple subtype4 layout checks; its clock value is informational and cannot change the authorized source or local timeline. It may name a different clock. |
| `ptp_msg_handle.c`: Delay/Pdelay handlers | Delay response interval/flags incomplete | Echo full requesting identity/sequence, receive timestamp from owned epoch; Pdelay response on event socket, post-send timestamp Follow_Up on general socket. Pdelay source reference explicitly calls its behavior a guess; PiAir follows standard two-step response semantics, with hardware applicability unverified. |
| `ptp_msg_handle.c`: Announce quality | OwnTone advertises GPS class6/100ns/variance0x436a, timeSource0x20 | Deliberate deviation: class248, accuracy0xfe, variance0xffff, internal oscillator0xa0; no UTC-valid, GPS, frequency/time traceability claim. PTP timescale flag matches reference, epoch remains monotonic, not UTC. Apple acceptance of this honest quality is a hardware assumption. At equal priority1, standard BMCA ranks class248 below a class6 clock; PiAir does not run BMCA or assume the receiver selected it. Priority1/2 remain128 rather than inventing a hardware-unverified election override. |
| `rtp_common.c`: `sync_packet_ptp_make` | Existing sender exposed NTP PT212 sync only | Shared realtime module now formats 28-byte PT215 announcements with current RTP position, monotonic nanoseconds, next RTP minus11025 and owned ClockID. TimingSession supplies its epoch/identity; PR 63 supplies scheduler positions. |
| PiAir `session.rs`, `wire.rs`, `profile.rs`, `buffered/` | Duplicating the NTP media engine risked changing working HomePod behavior | Consume existing authenticated Wire once; share offer/response/encoder code; add only PTP-owned UDP media/control preparation. NTP timing worker, default protocol selection, playback scheduler and buffered transport stay separate. |
| Reference startup accepts RTSP and prepares media without a receiver-adoption proof | Permanently false PTP media gate conflated timing observations with lock | Active `realtime_ready()` evaluates session authorization, completed stream negotiation, connected transport, event health, fresh peer liveness and healthy local timing. It can succeed; it asserts no remote synchronization/adoption. |

## Implemented ownership and sequence

1. Explicit CLI confirmation authorizes only the selected, allowlisted receiver
   and this no-audio operation. Default is StoredHap; explicit
   `--experimental-owntone-transient` retains its existing selection/policy.
2. Authenticate once on the selected control socket. StoredHap must complete
   signature-checked pair-verify and directional control-key installation.
   Transfer the exclusive Wire, cancellation flag, CSeq and encryption state;
   allocate no NTP service and never retry with another protocol.
3. Encrypted GET `/info`: validate deviceID, PTP/realtime capability and access
   policy. Discovery is not substituted for authenticated policy.
4. Initial PTP SETUP: sessionUUID, signed ClockID, timingPeerInfo/list and local
   address. Bind returned Session token presence for all later requests. Validate
   returned event port, ClockID and timing address before attaching resources.
5. Attach authenticated event service and UDP319/320 timing sockets on the RTSP
   route's local interface. Both connected sockets retain IP and channel-port
   filtering. Identity port numbers such as **32969** are the final two octets
   of sourcePortIdentity, not a new arbitrary UDP destination.
6. Send empty RECORD; validate acknowledgement. Send SETPEERS with the existing
   receiver/local address list and `/peer-list-changed` content type.
7. Bind UDP media/control sockets without an NTP socket; consume the verified pairing audio key into the existing
   realtime Encoder; offer type96 with shared key, connection ID and bound
   local control port over authenticated SETUP. Validate response before
   connecting to the selected control receiver's dataPort/controlPort. The
   separate timing address never redirects the media transport.
8. Observe for the requested 5–20 seconds while maintaining timing. Each control
   request has a scoped PTP worker. No speculative additional control requests:
   the verified reference requires none beyond stream SETUP except its omitted
   volume step. Do not claim that hardware acceptance has been established.
9. Bounded authenticated TEARDOWN with the original session binding. Timing loss
   does not suppress an authorized cleanup request. Revocation forbids further
   network writes, including TEARDOWN; report remote expiry unconfirmed. Release
   sockets, stop/join workers, erase encoder key, close Wire and clear token even
   when remote cleanup fails. The control cleanup request budget is 250 milliseconds; workers are joined locally.

The retained buffered `SampleTimeline` is 48 kHz and is **not** used for realtime
RTP. `TimingSession::realtime_sync` obtains monotonic timestamp and ClockID from
its owned grandmaster and calls the shared realtime serializer; it cannot accept
caller-supplied clocks. PR 63's existing 44.1 kHz scheduler supplies corresponding
current/next RTP positions. No RTP or RTCP send is exposed on PR 62's prepared
transport. This is an intentional no-audio boundary, not an impossible timing gate.

## Operational readiness and evidence

`realtime_ready()` requires all of these on the active owner:

- Uncancelled explicit operation; persisted selection/allowlist/credential lease
  still valid; live authenticated control owner and healthy event service.
- Valid initial SETUP/session binding, RECORD and SETPEERS acknowledgements,
  validated type96 stream SETUP, retained encoder/verified key, connected UDP transport.
- Continuous local monotonic timing worker, successfully sent Sync/Follow_Up,
  Announce and Signaling, plus recent validated authorized peer traffic.

Readiness can become true and is revoked by cancellation, timing loss, event or
control failure, selection/credential revocation or cleanup. A historical true
snapshot is never a reusable send grant. PR 63 must check readiness on every
media/control operation and run the existing bounded cleanup on failure.

PTP traffic is IP/ClockID authorized by authenticated RTSP, **not individually
cryptographically authenticated**. Fresh peer messages establish liveness only.
RTSP acceptance establishes control acceptance only. Local successful UDP sends
establish local socket acceptance only. None proves remote clock lock, adopted
GM, measured offset, precision, audio acceptance or audible output.

Replay history uses forward-half-range 16-bit sequence arithmetic per message
kind and port; wrap is supported. History is never evicted within a session.
A 64-port resource bound prevents unbounded allocation; this is an implementation
resource limit, not an Apple port-role restriction. Malformed, stale, reflected,
wrong-clock/channel/target packets cannot refresh liveness or consume a sequence.
Incoming two-step Syncs have independently correlated Follow_Up pairs. The
existing 30-second first-peer deadline and three-second poll/peer-silence budgets
remain explicit local health policy, not measured receiver lock requirements.

The CLI reports RTSP stage outcomes, attempted/last semantically accepted stage,
selected type and negotiated ports, packet send/receive/rejection counters,
ClockID and peer-role validation policy, historical timing health/readiness,
unverified remote synchronization, disabled audio, TEARDOWN and local cleanup.
Verbose packet rejections retain existing bounded header-only diagnostics.

## Tests and hardware-dependent assumptions

Tests exercise actual bounded packet bytes and encrypted RTSP framing, not
success booleans. Coverage includes type96 offer/key/ports and ordered control
requests, tokenless/token-bearing binding, invalid stream parameters, disconnect,
multiple concurrent PTP identities (including port32969), sequence wrap/replay,
Sync/Follow_Up correlation, Signaling organization TLVs, truthful Announce,
Delay/Pdelay routing, cancellation/revocation/timing loss and bounded cleanup.
The existing HomePod NTP handshake/playback/feedback/nonce suites remain required.

Earlier user reports establish Apple TV acceptance through SETPEERS and traffic
from multiple PTP identities. No raw hardware capture was provided in this task.
The 32969 fixture and traffic sequences are clearly labeled regression models;
they are not fabricated byte-for-byte captures. No Raspberry Pi or Apple TV is
reachable from this cloud environment. Stream SETUP acceptance, free-running
clock-quality acceptance, long-lived peer cadence and any additional
firmware-specific requests need the hardware run below. Private Apple TLV
semantics and remote synchronization remain unverified. Never substitute a
GPS-quality lie, wider peer authority or automatic NTP fallback for that evidence.

## PR 63 media integration

Reuse this owner, encoder/key, transport, readiness predicate and timing epoch.
Connect the existing WAV reader/ALAC16 encoder, absolute 44.1 kHz RTP scheduler,
nonce/cache/retransmission and feedback handling; add explicit playback
confirmation. Use `TimingSession::realtime_sync` for initial/periodic PT215
announcements with scheduler-correlated RTP positions and this clock's monotonic
nanoseconds. Retain the existing playback lead/drain policy and validate sample
wrap and latency on hardware. Keep timing polling independent while scheduling,
feeding WAV data, processing feedback or issuing RTSP requests. Add authorized
media/control sends to the prepared transport with per-operation checks; never
recreate the session, key or clock to begin playback. No buffered anchor or TCP
media conversion is required.

## Exact Raspberry Pi deployment and verification

Run on the Pi under the same user/configuration used to pair, allowlist and
select the receiver. Supply the selected ID interactively; no ID/IP is hardcoded.

```bash
cd ~/PiAir
git fetch origin feat/ptp-realtime-negotiation
git switch --detach origin/feat/ptp-realtime-negotiation
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
sudo setcap cap_net_bind_service=+ep ./target/release/piair
read -r -p 'Exact selected receiver ID: ' PIAIR_RECEIVER_ID
./target/release/piair -v ptp-negotiation-check "$PIAIR_RECEIVER_ID" --observation-seconds 10
```

Reapply setcap after rebuilding. UDP319/320 must be free on the selected local
interface; a bind failure cleans up without alternate ports. Confirm the
interactive no-audio authorization. For an already selected transient receiver,
append `--experimental-owntone-transient` explicitly; never switch authentication
automatically. Independently capture UDP319/320 and the negotiated media/control
ports if needed; protect pairing credentials and decrypted RTSP payloads.

Expected: authenticated `/info`, initial SETUP, RECORD, SETPEERS and realtime
stream SETUP accepted; type96 with valid distinct ports; periodic paired Sync /
Follow_Up, Announce and Signaling; validated receiver identities; completed
observation with operational readiness true; synchronization unverified; no
media datagrams/volume changes; acknowledged TEARDOWN and released local
resources. Repeat with Ctrl-C and receiver disconnect to verify bounded failure
cleanup. Rejection/loss must fail the command and release locally; never report
hardware playback or receiver synchronization merely from these outcomes.
