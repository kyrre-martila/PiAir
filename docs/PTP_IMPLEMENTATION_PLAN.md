# PTP integration foundation (Prompt 57)

## Status and scope

Reviewed PiAir main `2cb41c9eb956e3ce2694e7c4bcf0f0adbc1fc57a` (#56).
This change provides reusable session ownership, not Apple TV audio playback.
No receiver or Raspberry Pi hardware was used. UDP send success, RTSP 200,
accepted anchors and `PeerObserved` do not prove synchronization or audibility.
`Grandmaster::media_permitted()` remains unconditionally false.

Relevant history establishes why a replacement PTP stack would be wrong:

- `5e6cc2a`: packet/time/control/grandmaster foundation and independent fixtures.
- `b5940bf`: mandatory authenticated receiver authorization and per-packet leases.
- `4e71583`: buffered lifecycle, bounded cleanup and initial SETUP diagnostics.
- `3670314`: authenticated timing address delegation and route-source binding.
- `87e60a8`: message-specific HomePod zero control-field compatibility.
- `8944ef0`: explicit missing clock-adoption evidence; no fabricated lock signal.
- `b431281`: tokenless control sessions retain an exclusive Wire binding.
- #54–#56: stored-HAP WAV, silence diagnostics and exclusive feedback ownership.

Existing capabilities: bounded PTP v2 parsing, domain/identity/sequence checks,
Sync plus matching Follow_Up, Announce, Delay_Req/Resp, peer-delay responses,
opaque general-channel observations, session-relative monotonic epoch, integer
Q64 network time, 48 kHz absolute sample mapping, typed SETPEERS and anchor
control. The standalone follower estimator is not a receiver clock adoption
protocol. It is not used to authorize media.

`wire::start_ptp` binds the RTSP local address to UDP 319/320, connects each
socket to one validated authenticated SETUP destination, and retains a PtpLease
for the original receiver. Address delegation is not authorization delegation.
See [authenticated-ptp-peers.md](authenticated-ptp-peers.md) for exact selection,
route and IPv6 limits. No peer retry, UDP registration or NTP fallback is added.

The working WAV path remains `session::Session` → `profile::select_ntp` →
`realtime::Clock`/Encoder → early NTP responder and RTP scheduler. Its format is
44.1 kHz ALAC16 stereo. `buffered` is a distinct type103 ALAC24/48 kHz transport;
`media` is another observed packet model, not a negotiated profile. Their packet
layouts, rates and sequence widths must not be mixed. Authentication capabilities
in `compatibility` do not establish playback/timing capabilities.

## Concrete integration boundary

`Session::into_ptp_foundation` consumes a pristine authenticated WAV session and
moves its existing Wire into `lifecycle::Session::from_authenticated`. It retains
receiver authorization, HAP/transient authentication, CSeq, cipher state and the
same cancellation Arc. It performs no request, redial, pairing, SETUP or audio.
It rejects terminal, diagnostic, cancelled and already negotiated NTP owners.
The target installs cancellation on control requests and rechecks a mandatory
PTP lease before accepting the transfer. `setup_authenticated` is an explicit
initial PTP diagnostic entry; it is not called by existing WAV commands.
Authenticated /info identity, features, format and policy validation still
precede initial SETUP, which alone can establish its session token and peer ID.

`ptp::session::TimingSession` now owns the existing Service in the active buffered
lifecycle. Phases are Observing → PeerObserved, with terminal Failed/Closed.
There is no transition to Locked or media readiness. Each poll uses the existing
Service/PtpLease checks and grandmaster deadlines: setup <=30 s, peer liveness
and polling gap <=3 s; bounded work per channel. Cancellation, revocation or
control failure close both sockets and discard the mapping. Drop and explicit
shutdown are idempotent. Failed owners cannot restart.

`during_control` services PTP in a scoped worker while the exclusive Wire owner
makes its bounded request. The worker joins before acceptance or cleanup.
Wire retains request budgets/cancellation and the lifecycle retains token checks,
event-worker ownership and bounded encrypted TEARDOWN. The owner cannot interrupt
arbitrary blocking closures: callers must use bounded Wire requests, as the
integrated lifecycle does. Timing failure is propagated after that request ends.

Paused diagnostic anchors now capture the owned grandmaster's ID, generation
and network epoch together. A second mapping is rejected; stale, replaced or
expired clocks cannot validate an anchor. This uses existing `AnchorRequest`,
`SampleTimeline` and acknowledgement validation rather than duplicating RTSP or
clock math. Live `Rate::Play` control and media remain gated. The lower-level
borrowed `control::Control` remains available; the lifecycle uses its existing
session-token-aware request path with the same typed control encoders/validators.
PtpLease now also explicitly compares its receiver ID to its authorization pin.

## OwnTone comparison: verified source facts

Reference snapshot: upstream **owntone/owntone-server**
[`8ed84f2b3cba25d3de65e05660d59ce034b5adab`](https://github.com/owntone/owntone-server/tree/8ed84f2b3cba25d3de65e05660d59ce034b5adab).
Function citations below refer to that immutable tree. This is a protocol
comparison only; no GPL implementation was copied, translated or linked.
OwnTone Mini in older PiAir research is a different reference; its extended
buffered/anchor sequence must not be attributed to this upstream snapshot.

| Area | Verified OwnTone functions and behavior | PiAir status / difference |
| --- | --- | --- |
| Capability selection | `src/outputs/airplay.c`: feature table names bit41 SupportsPTP; `airplay_device_cb` selects it subject to receiver/global PTP disable policy; `airplay_init` disables PTP if daemon initialization fails. `master_session_make` partitions RTP masters by quality and timing mode. | Keep explicit authenticated capability validation and fail closed. Do not copy upstream fallback policy. |
| Clock identity | `src/ptpd.c`: `ptpd_clock_id_get` delegates to `airptp_clock_id_get`. `src/libairptp/src/airptp.c`: `airptp_daemon_start` ORs the seed with `0xFFFF000000000000`; getter returns daemon clock ID. | One random valid local ID per lifecycle; same ID in SETUP, PTP packets and mapping, with process-unique generation. No need to copy upstream ID construction. |
| Ports and ownership | `ptpd_find_or_bind` can reuse a host daemon or bind a private service. `airptp_daemon_bind` binds event/general services (default319/320); `airptp_ports_override` permits overrides. | Session-owned connected sockets only; no shared daemon or privileged-port workaround. Bind failures fail closed. |
| Initial SETUP | `payload_make_setup_session_ptp` emits PTP, sessionUUID, name/deviceID, timingPeerInfo and timingPeerList; peer has ID UUID, DeviceType0, signed ClockID, local Addresses and SupportsClockPortMatchingOverride=false. Also emits MAC/group/MRP fields. | Existing initial_offer includes core timing fields; optional override/MAC/group/MRP semantics are not established for PiAir and are not guessed. Signed bplist clock IDs preserve all bits. |
| Peer binding | `response_handler_setup_session` invokes `handle_timingpeerinfo`; it walks Addresses of active family, adds IPv6 scope and calls `ptpd_slave_add` until successful. `airptp_peer_add` registers address-derived peer ID through local daemon signaling. | Require authenticated ClockID and bounded numeric address list, original receiver authorization, one selected destination, matching route source, no private UDP registration. Upstream functions reviewed here do not enforce PiAir's receiver-bound ClockID lease policy. |
| Sync/Follow_Up | `src/libairptp/src/ptp_msg_handle.c`: `ptp_msg_sync_send` emits two-step Sync and Follow_Up; `current_time_get` uses CLOCK_MONOTONIC. `msg_sync_follow_up_make` emits standard and Apple clock-ID TLVs. `sync_handle`/`follow_up_handle` parse/log incoming timestamps. | Existing PiAir fixed GM sends timestamped pairs; inbound Sync does not elect a clock or refresh delay liveness. Follow_Up uses the actual userspace send completion time; no kernel timestamp accuracy claim. |
| Announce and delays | `msg_announce_make` advertises high clock quality; `announce_handle` explicitly assumes local GM and otherwise logs/ignores Announce. `delay_msg_handle` replies on general service; `pdelay_msg_handle` emits response/follow-up. | Keep truthful free-running oscillator quality. Bind request source to authenticated ClockID and pinned port identity; reject replay. Delay traffic proves activity only. |
| RTP clock mapping | `master_session_make` supplies daemon clock ID to `rtp_session_new`; `packets_sync_send` passes monotonic sample/time correspondence to `rtp_sync_packet_next`. In `src/outputs/rtp_common.c`, `sync_packet_ptp_make` emits 28-byte type0xd7, sample position, 64-bit nanoseconds, earliest sample expression `pos-11025`, clock ID. NTP uses a different packet. | Existing realtime::sync is type0xd4/NTP and must not be reused as PTP. New timing owner only binds 48 kHz mapping/paused anchor. Upstream RTP latency constant is profile-specific, not a validated PiAir buffered value. |
| Control and teardown | `airplay_seq_request` orders initial SETUP, RECORD, SETPEERS, stream SETUP, volume. `payload_make_setpeers` includes receiver/local addresses. Stop/failure sequences issue TEARDOWN; `session_free` calls `ptpd_slave_remove`; `airplay_deinit` frees sessions before `ptpd_deinit`. | Existing staged buffered diagnostics also register commands/audioMode and can test paused anchors; this upstream table is not evidence for those extensions. Preserve bounded TEARDOWN, no new volume or audio operation. |

Facts above describe sender behavior, not protocol requirements or a receiver's
servo state. Neither OwnTone UDP sends nor its fixed-GM assumption establish
an authenticated clock adoption report. See
[CLOCK_ADOPTION_INVESTIGATION.md](CLOCK_ADOPTION_INVESTIGATION.md).

## Remaining integration points for PRs #58–#61

These are proposed implementation boundaries for the requested follow-ups,
not a claim that those PRs already exist or are validated:

1. **#58: authenticated timing/RTSP selection.** After the existing same-socket
   authentication, reconcile authenticated /info with pinned selection and choose
   an explicit supported timing/media profile before allocating NTP resources.
   Use `into_ptp_foundation` only for that PTP branch and `setup_authenticated`
   for initial SETUP. Preserve transient and stored-HAP operation authorization,
   tokenless binding, session headers, event ownership and cleanup. Never retry
   as NTP on a PTP failure. Test full handoff through negotiation and revocation.
2. **#59: PTP RTP time correspondence.** Determine which Apple TV profile is
   actually supported (realtime UDP vs buffered type103) before choosing packet
   layout. Implement its PTP time announcements/anchors from the same owned
   identity, epoch and absolute samples; test byte fixtures, sample rate,
   latency, wrap and clock loss. Do not convert NTP wall-clock bits into PTP.
   Establish receiver clock-adoption evidence and the authorized media-grant
   semantics before changing `media_permitted`; otherwise remain diagnostic-only.
3. **#60: WAV media engine integration.** Connect existing WAV decoding to the
   selected codec/rate, bounded queue and transport after full negotiation and
   a reviewed media grant. Reconcile S16/44.1 kHz input with ALAC24/48 kHz only
   through explicit conversion/resampling policy. Preserve nonce lifetime,
   exclusive feedback/Wire ownership, backpressure and cancellation. No sound
   claim from packet counts. Keep existing NTP branch and regression fixtures.
4. **#61: hardware validation and release gate.** Collect correlated Raspberry Pi
   control/PTP/RTP evidence and independently observed Apple TV audio, document
   firmware/profile/format and start/stop/loss behavior. Repeat HomePod NTP WAV
   and transient/stored-HAP regressions. Enable public PTP WAV selection only
   after protocol and authorization gates have evidence and tests.

Unresolved: Apple TV's accepted stream profile/format/encryption; mandatory
optional SETUP fields; whether arbitrary session-relative monotonic epochs work
for all intended receivers; private Apple TLV/selected-GM identity semantics;
clock quality acceptance without false GPS/traceability claims; authenticated,
fresh and revocable clock adoption evidence; unicast signaling requirements;
profile-specific RTP latency and time-announce/anchor requirements. The upstream
realtime 0xd7 packet and Mini buffered anchors are not interchangeable evidence.
IPv6 link-local scope, multihoming and shared daemon integration remain unsupported.

## Validation and Raspberry Pi checklist

Required local checks: `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo test`.
New pure tests require only temporary files/monotonic instants for authorization,
revocation, receiver identity, mapping generation/epoch, handoff guards and timeout.
Integrated control, cancellation and socket-release tests use existing unprivileged
loopback fixtures, with no external network, Apple TV or UDP319/320 access.
Existing NTP WAV, transient, stored-HAP, feedback and CLI tests remain the regression
suite. A successful test run does not validate any hardware protocol assumption.

Hardware checklist (not performed):

- Confirm authorized receiver ID, endpoint, standalone topology, selected method
  and firmware. Build release on Pi and establish the working NTP/HomePod WAV
  baseline; retain stored-HAP Apple TV credential behavior.
- Check UDP319/320 availability and chosen interface. Use existing explicit PTP
  diagnostics only; capture both channels and encrypted control outcomes locally
  without publishing credentials, keys, tokens or decrypted plists.
- Correlate authenticated ClockID, SETUP identity, Sync/Follow_Up sequences,
  Announce and Delay responses; verify one consistent epoch and destination.
  Record activity separately from adoption/lock and audible output.
- Exercise cancel, revoked approval, receiver disconnect and timing expiry;
  verify bounded encrypted TEARDOWN where still authorized, released sockets,
  zero media bytes and explicit unconfirmed remote expiry on cleanup failure.
- After #58–#60 gates are complete, test a short known signal at an explicit safe
  volume, independently verify sound and its timing, then longer WAV, stop and
  loss. Repeat NTP HomePod and stored-HAP tests. Do not run this future playback
  step using this foundation alone.
