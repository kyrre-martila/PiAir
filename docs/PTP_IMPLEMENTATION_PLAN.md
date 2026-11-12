# Authenticated PTP negotiation (Prompts 57–58)

## Status and scope

Foundation reviewed main `2cb41c9eb956e3ce2694e7c4bcf0f0adbc1fc57a` (#56);
Prompt 58 builds on merged #57 (`79f71c129eee07b7d06ab8d71af19d91669e6b33`).
The explicit CLI below provides authenticated PTP negotiation, not Apple TV audio playback.
No receiver or Raspberry Pi hardware was used by this implementation session;
user-supplied hardware evidence is recorded below. UDP send success, RTSP 200,
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
| Sync/Follow_Up | `src/libairptp/src/ptp_msg_handle.c`: `ptp_msg_sync_send` emits two-step Sync and Follow_Up; `current_time_get` uses CLOCK_MONOTONIC. `msg_sync_follow_up_make` emits standard and Apple clock-ID TLVs. `sync_handle`/`follow_up_handle` parse/log incoming timestamps. | Existing PiAir fixed GM sends timestamped pairs; inbound Sync does not elect a clock; fresh validated observations refresh peer liveness (Prompt 61). Follow_Up uses the actual userspace send completion time; no kernel timestamp accuracy claim. |
| Announce and delays | `msg_announce_make` advertises high clock quality; `announce_handle` explicitly assumes local GM and otherwise logs/ignores Announce. `delay_msg_handle` replies on general service; `pdelay_msg_handle` emits response/follow-up. | Keep truthful free-running oscillator quality. Bind request source to authenticated ClockID and pinned port identity; reject replay. Delay traffic proves activity only. |
| RTP clock mapping | `master_session_make` supplies daemon clock ID to `rtp_session_new`; `packets_sync_send` passes monotonic sample/time correspondence to `rtp_sync_packet_next`. In `src/outputs/rtp_common.c`, `sync_packet_ptp_make` emits 28-byte type0xd7, sample position, 64-bit nanoseconds, earliest sample expression `pos-11025`, clock ID. NTP uses a different packet. | Existing realtime::sync is type0xd4/NTP and must not be reused as PTP. New timing owner only binds 48 kHz mapping/paused anchor. Upstream RTP latency constant is profile-specific, not a validated PiAir buffered value. |
| Control and teardown | `airplay_seq_request` orders initial SETUP, RECORD, SETPEERS, stream SETUP, volume. `payload_make_setpeers` includes receiver/local addresses. Stop/failure sequences issue TEARDOWN; `session_free` calls `ptpd_slave_remove`; `airplay_deinit` frees sessions before `ptpd_deinit`. | Existing staged buffered diagnostics also register commands/audioMode and can test paused anchors; this upstream table is not evidence for those extensions. Preserve bounded TEARDOWN, no new volume or audio operation. |

Facts above describe sender behavior, not protocol requirements or a receiver's
servo state. Neither OwnTone UDP sends nor its fixed-GM assumption establish
an authenticated clock adoption report. See
[CLOCK_ADOPTION_INVESTIGATION.md](CLOCK_ADOPTION_INVESTIGATION.md).

## Integration roadmap for PRs #58–#61

The #58 negotiation boundary is implemented below. Remaining media boundaries
are proposed follow-ups, not a claim of validated playback:

1. **#58: authenticated timing/RTSP selection (implemented).** After the existing same-socket
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

## Prompt 58: authenticated negotiation-only CLI

`ptp-negotiation-check` explicitly selects the PTP branch. It requires the exact
persisted selected/allowlisted receiver, existing stored-HAP credentials and
terminal confirmation. The operation derives StoredHap using the existing WAV
authorization helper, authenticates once, and consumes the same Session/Wire via
`into_ptp_foundation` and `setup_authenticated`. No NTP resources are allocated.
Explicit `--experimental-owntone-transient` instead requires a persisted transient
selection and compatible existing policy; no fallback or automatic pairing occurs.
Cancellation is installed before authentication and retained through the handoff.

The actual OwnTone files were rechecked at
`8ed84f2b3cba25d3de65e05660d59ce034b5adab` for this change: initial SETUP's PTP,
signed ClockID, timingPeerInfo/list and local addresses; response peer selection;
`ptpd.c` clock/peer delegation; libairptp event/general 319/320 binding and
Sync/Follow_Up/Announce/delay handling. The initial offer now explicitly advertises
`SupportsClockPortMatchingOverride=false`. MAC/group/MRP extensions remain omitted
because this diagnostic does not implement those roles. No GPL code was copied.
The timing-only progression reuses the existing buffered lifecycle's RECORD then
SETPEERS encoders and validation, stopping before its stream/key/command/audioMode
or anchor stages. It never opens media sockets, sends audio, or changes volume.

On the Raspberry Pi, after allowlisting, pairing and explicitly selecting the
Apple TV with the existing commands, run (replace the example with its exact ID):

```sh
cargo build --release
sudo setcap cap_net_bind_service=+ep ./target/release/piair
./target/release/piair ptp-negotiation-check device:02d899c1833f --observation-seconds 10
```

Use the same user and configuration as pairing/selection. The capability permits
binding UDP319/320 without running the CLI as root; reapply it after rebuilding.
Both ports must be free on the RTSP route's local address. A bind conflict fails
and triggers bounded authorized TEARDOWN, with no alternate port or NTP retry.
The default observation is ten seconds (explicit range 5–20), servicing timing
through RECORD, SETPEERS and cleanup. Existing 30-second initial peer deadline
and three-second peer/poll liveness rules still apply; loss may end the diagnostic
early. Zero delay exchanges do not exclude observations of other validated PTP
message types; no receive count proves synchronization.

Expected final evidence is `initial SETUP RTSP accepted: true`,
`last accepted: SETPEERS`, packet evidence containing
successful Sync, Follow_Up and Announce sends, validated pinned-ClockID delay
requests and successful delay responses, `PeerObserved` after fresh validated peer
traffic, `TEARDOWN: acknowledged`, and `audio bytes: 0`. General-channel receive
counts are validated observations, not adoption reports. Send counts prove only
local socket acceptance. Receiver clock adoption stays `unverified` and media
readiness stays `blocked` even if every request succeeds. Capture UDP319/320 to
independently correlate sequences and identities; never publish pairing secrets
or decrypted control payloads. Cancellation, revocation, disconnect or timeout
may instead report a failed session or unconfirmed remote expiry; sockets are
released locally. No Apple TV or Raspberry Pi hardware validation was performed.

For PR #59's PTP-synchronized WAV test, first establish the actual Apple TV media
profile, clock-adoption evidence and a revocable media grant. Implement the
profile's RTP time correspondence/anchors from this same ClockID and epoch,
explicit codec/rate conversion and transport/feedback ownership. Test packet
fixtures, latency, sample wrap and loss before permitting any WAV bytes. An
accepted SETPEERS, elapsed observation or `PeerObserved` cannot open that gate.
The existing NTP WAV path and its regression suite remain unchanged.

## Prompt 59: exact authenticated access-policy rejection (historical)

Reported physical result on Apple TV Stua (AppleTV6,2, tvOS 26.6.1): stored-HAP
pair-verify M1/M2 and M3/M4 succeeded, encrypted GET /info returned HTTP 200,
and buffered format validation passed. `lifecycle::validate_info` then rejected
`unsupported authenticated access policy` before initial PTP SETUP. This is a
user-reported result; no hardware was contacted for this change.

On main `5faae58` (merged #58), that exact error has two possible predicates:
`statusFlags & ((1 << 3) | (1 << 7) | (1 << 9)) != 0`, or any present `acl`,
`act`, `accessControlLevel`, `accessControlType` is nonzero. The log cannot tell
which failed. Reported discovery `flags=99908` needs numeric provenance: decimal diagnostic
output is `0x18644` (including bit 9, consistent with the report), whereas a
literal TXT string `99908` parses as hexadecimal `0x99908` (including bit 3,
without bit 9). Neither is a captured authenticated /info value.
Do not substitute it for `statusFlags` or claim bit 9 caused this rejection.

Actual OwnTone `src/outputs/airplay.c` was inspected at
[`8ed84f2b3cba25d3de65e05660d59ce034b5adab`](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c):
its flag definitions identify bit 3 as PIN required, bit 7 as password required,
and bit 9 as one-time pairing required. `response_handler_info_generic` selects
normal HAP pair-verify when bit 9 is set and a stored auth key exists, otherwise
PIN enrollment. Its bit-3 branch discards the stored key and requests PIN setup.
These are pre-authentication sequence decisions, not evidence that every such
flag or combination is safe after verification, nor an implementation of Home
membership authorization. No OwnTone implementation code was copied.

PiAir `compatibility.rs` already permits persistent HAP with discovery bits 3/9,
rejects those bits for transient authentication, and pins them across selection.
`planning.rs::evaluate_airplay_auth` similarly distinguishes StoredHap from
Transient for authenticated bits 3/9. StoredHap still means successful pinned
receiver identity verification on the same encrypted control socket; an
advertisement or transient SRP success cannot confer that identity. The stricter
PTP lifecycle gate predates #58 and currently applies to both methods. Without
Stua's authenticated field values, changing it would guess at the actual failure.

This change therefore preserves every policy predicate. A bounded debug summary
reports only the fixed policy field names, integer values (decimal and hex),
booleans, absence and invalid-type markers. Rejection errors identify the exact
field and expected value; status rejection includes the raw flags and rejected
mask with bit labels. Missing/negative/malformed values remain errors. Strings,
data, arrays and dictionaries are never rendered, and no keys, identities,
credentials, session tokens or complete decrypted plists are logged. Group/proxy
checks, allowlist/selection binding, stored-HAP verification, cancellation and
fail-closed cleanup remain in force. Existing NTP WAV policy is untouched.

After rebuilding and reapplying UDP319/320 bind capability as described above,
retest under the same paired/selected user configuration with exactly:

```sh
./target/release/piair -v ptp-negotiation-check device:02d899c1833f --observation-seconds 10
```

Capture `authenticated /info access policy` and the exact rejected field/value.
A synthetic `statusFlags=0x99908` regression intentionally still rejects mask
`0x8`; decimal `99908` rejects mask `0x200`. Neither is an
authenticated hardware fixture. Nonzero ACL/type or group/proxy fields
must not be waived because pair-verify succeeded. Any later bit-9 exception needs
captured authenticated evidence and method-specific tests, while preserving PIN,
password and unresolved authorization restrictions. Rejection before SETUP closes
the encrypted control socket and allocates no timing/media resources; no remote
session exists to TEARDOWN. Existing timing-only success/cleanup tests remain.
This command is negotiation-only with zero audio bytes. No PTP synchronization,
clock adoption, PTP WAV playback or audible playback is claimed; default protocol
selection and the HomePod NTP WAV path are unchanged.


## Prompt 60: verified StoredHap one-time pairing policy

Confirmed user-supplied hardware evidence for Stua (AppleTV6,2, tvOS 26.6.1):
StoredHap pair-verify M1/M2 and M3/M4 each returned HTTP 200 successfully;
encrypted GET /info returned HTTP 200. Authenticated `statusFlags=99908`
(`0x18644`) rejected only mask `0x200` (bit 9). `acl`, `act`,
`accessControlLevel`, `accessControlType` and group/proxy fields were absent.
Buffered ALAC24/48000/stereo was supported. PTP SETUP was not yet attempted.
This supersedes Prompt 59's uncertainty about authenticated numeric provenance;
`0x99908` remains a distinct synthetic PIN-required rejection case.

Pair-verify now mints private StoredHap evidence only after checking the credential
receiver binding, pinned pairing identifier and Ed25519 signature, successful M4,
and installation of directional control keys. The same exclusive Wire owns that
evidence through transfer from the WAV foundation. Closing clears it; failed or
unencrypted wires cannot expose it. CLI method selection alone, transient SRP,
and verification using enrollment methods do not grant this exception.

Authenticated PTP /info permits bit 9 only with that evidence. It still requires
the exact receiver deviceID and rejects bit 3, bit 7, nonzero or malformed
access-control fields, malformed flags and unsupported group/proxy states.
Discovery, selection pinning, allowlisting, planning, cancellation, deadlines,
and cleanup are unchanged. Rejection before SETUP closes control and allocates no
timing or media resources. The HomePod NTP WAV path remains unchanged.

Next hardware validation: rebuild, restore UDP319/320 bind capability, and repeat
the explicit negotiation-only command above. Verify /info passes and capture the
initial PTP SETUP response, authenticated clock/address binding, timing socket
startup, packet evidence and bounded cleanup. Then validate later diagnostic
stages as documented above. No successful PTP synchronization, receiver clock
adoption or audible playback is established by this evidence. Media readiness
remains blocked, audio bytes remain zero and default protocol selection is unchanged.

## Prompt 61: peer identity and observation liveness

User-reported AppleTV6,2 / tvOS 26.6.1 evidence (device:02d899c1833f,
receiver 192.168.1.236, sender 192.168.1.43): StoredHap and authenticated
GET /info, SETUP, RECORD, SETPEERS and TEARDOWN succeeded with HTTP 200.
Before failure, 24 Sync, 24 Follow_Up and 3 Announce were sent; one Delay_Req
received one response. Additional incoming Sync, Follow_Up, Announce and Signaling
were observed. Main 30e3423 rejected general traffic against the first
Delay_Req's entire sourcePortIdentity, rejected incoming Sync, and expired after
three seconds without another delay request. Audio bytes were zero. These are
user reports, not captures generated or hardware tests performed in this PR.

Reference review: OwnTone 8ed84f2b3cba25d3de65e05660d59ce034b5adab,
`src/libairptp/src/ptp_msg_handle.c` dispatches Sync, Follow_Up, Announce,
Delay_Req, Pdelay_Req and Signaling separately. Sync/Follow_Up handlers observe
headers/timestamps; Announce does not elect another master. Delay responses echo
the requesting port identity. Neither those handlers nor `airptp.c` establishes
that all types share one source port. `src/ptpd.c` delegates clock/peer lifecycle;
`src/outputs/airplay.c` supplies PTP ClockID/addresses in timingPeerInfo/list and
registers the SETUP-selected peer. Its signaling implementation can mutate daemon
peers; PiAir deliberately does not implement that behavior. Reference only:
no GPL implementation code was copied.

IEEE sourcePortIdentity separates an 8-byte clockIdentity from a 2-byte
portNumber. A clock may have multiple ports; the supplied hardware report shows
that cross-type equality is unsafe, but supplies no exact port values. PiAir
therefore retains the authorized connected UDP endpoint and authenticated clock
identity, parses the nonzero portNumber explicitly, and pins the first validated
port independently for Delay_Req, Pdelay_Req, Sync, Announce and Signaling. Later
port changes within a role fail closed. Follow_Up must match a pending two-step
Sync's exact source port and sequence within three seconds; duplicates,
unmatched/out-of-order Follow_Up and sequences outside the forward half-range
are rejected. The pending slot and per-type sequence/port tables are fixed-size.
This is a conservative observation policy, not a claim about Apple's internal
port allocation. Multiple simultaneous ports within a role remain unsupported.

Valid fresh Delay_Req/Pdelay_Req, Sync, correlated Follow_Up, Announce and
correctly targeted Signaling establish/refresh **peer liveness**. All packet
length/domain/reserved/flag/timestamp/TLV checks still apply before acceptance.
Signaling TLVs remain inert and cannot change peers, destinations, authorization,
intervals or timing policy. Rejected, reflected, replayed, wrong-clock,
wrong-channel and wrong-target packets cannot refresh liveness. Connected sockets
retain IP/UDP-port filtering; no multicast, arbitrary peers or destinations are
introduced. Rejection logs have a lifetime bound and expose only header message
type, clock identity and port number, never TLV/control payloads.

The independent budgets are unchanged: initial peer setup deadline (30 seconds
in the diagnostic), local poll/monotonic continuity (three seconds), and validated
peer silence (three seconds after first observation). The CLI observation has
its own requested 5–20 second bound. Thus one Delay_Req followed by fresh valid
peer traffic can complete ten seconds, while actual silence still terminates.
No timeout was globally increased. Cancellation, lease revocation and socket
errors remain terminal, release resources, and preserve authorized bounded
TEARDOWN attempts. Observation completion, per-type receive/send counts,
rejections, peer liveness snapshot, RTSP acceptance and TEARDOWN are reported
separately. Completed observation does not prove receiver synchronization.

Local grandmaster identity, epoch and generation never change on reception.
No BMCA election, servo submission or remote clock adoption is performed.
`PeerObserved` means traffic only; adoption and synchronization stay unverified,
media readiness blocked and audio bytes zero. Existing HomePod NTP behavior and
default protocol selection are unchanged.

Remaining hardware uncertainties before WAV playback: actual source port values
and ordering, any role-port changes, repeated signaling sequence semantics,
private TLV clock/adoption semantics, accepted media profile, clock quality and
revocable synchronization evidence. The existing conflicting Apple clock TLV
restriction is retained; a TLV naming a different clock is not proof of adoption.
These need captures and protocol evidence before relaxing restrictions or sending
PTP audio. Synthetic regression fixtures model the reported sequence; they are
not byte-for-byte hardware captures.

Exact hardware retest command (after rebuilding and restoring bind capability):

```sh
./target/release/piair -v ptp-negotiation-check device:02d899c1833f --observation-seconds 10
```

Verify completed observation, fresh per-type traffic, rejected counts/reasons,
unchanged local clock, acknowledged TEARDOWN, zero audio and blocked media.
No hardware retest was performed in this cloud environment.

### PR 61 review: source port, Sync policy and cleanup snapshots

IEEE 1588-2008 section 7.5.2.3 numbers ordinary PTP ports starting at one;
zero is not an ordinary source port for these event/general observations.
`Packet::validate` already rejected the two zero source-port octets before any
receive handler ran. The rule now uses the parsed `port_number()` explicitly,
and the typed observation boundary independently rejects zero before updating
port pins, sequence tracking or liveness. Raw-wire and typed regression tests
cover all six accepted incoming message types; the nonzero documentation remains
accurate. Signaling wildcard *targets* are separate from source identities.

`Packet::decode` checks header, domain, reserved bytes, source identity, generic
flag mask, timestamp and bounded TLVs. Its generic flag mask and signed correction
field are deliberately broader than the supported Sync observation policy.
Incoming Sync now additionally requires exactly the ten-byte timestamp body,
zero correction, and only twoStepFlag (0x0200), unicastFlag (0x0400), and
ptpTimescale (0x0008). Both one-step and two-step observations remain supported.
Nonzero correction or extensions may be legitimate PTP in other profiles, but
PiAir does not implement their semantics and retains a safe restriction pending
protocol/capture evidence. Unsupported flags (including time-properties flags
belonging to Announce), corrections and extensions cannot pin a source port,
consume a sequence, establish pending Follow_Up correlation or refresh liveness.
Tests also inject malformed headers, timestamps and truncated Sync bytes.

The lifecycle stores the peer-liveness result at successful observation
completion, independently of the live timing service. Before cleanup on an
incomplete observation it instead saves the current checked liveness result.
`peer liveness at snapshot` is explicitly historical; it does not assert current
session validity after cancellation, revocation or shutdown. Final `PTP snapshot`
now reports the post-cleanup Shutdown state, while the saved observation result
and completion status remain intact. Successful token-bearing and tokenless
observation/TEARDOWN tests cover ordinary cleanup and a timing service already
shut down before authorized TEARDOWN. Timing shutdown never grants media;
authorization checks and zero-audio behavior are unchanged.
