# AirPlay 2 PTP and authenticated timing foundation

Current single-receiver status: [BUFFERED_LIFECYCLE.md](BUFFERED_LIFECYCLE.md)
implements an opt-in authenticated initial PTP SETUP diagnostic and bounded
cleanup. Live buffered playback remains closed because receiver clock adoption
is unverified. Historical PR #42 prerequisite lists below remain unresolved
except for that explicitly documented foundation. No verified audio is claimed.

Baseline: PR #40 merge `34ea81a3df3a5f520542ce32db1fe1052a0b8c82` on main. This is PR #41's timing foundation.
**Buffered media remains disabled. No audible playback or Apple hardware clock
lock has been verified.** Realtime NTP is unchanged. No daemon, system service,
system clock adjustment, multicast discovery, automatic pairing or group
connection is introduced.

## Ownership and message flow

`receiver/ptp/packet.rs` implements a bounded IEEE 1588-2008 packet codec.
`time.rs` provides integer timestamp arithmetic, the four-timestamp symmetric
path solution and an absolute 48 kHz sample mapping. `mod.rs` owns clock state,
a fixed grandmaster and a nonblocking, session-polled UDP service. `control.rs`
contains typed timing requests and acknowledgement validation. Anchors use the
`buffered::PtpAnchor` interface from PR #40; the complete `BufferedLifecycle`
trait intentionally still has no live implementation.

Only `Wire::start_ptp` can create production timing sockets. It checks existing
authorization and authenticated control keys, binds the control socket's local
literal address on UDP 319/320, and connects only to the same receiver address.
Every service requires a non-optional `PtpLease` capability minted only by a live
Wire with authenticated control keys and concrete persisted/operation authorization.
Its fields and constructor are private to Wire; the optional legacy lease is not
accepted. `Service::attach` is restricted to the receiver implementation and
requires that capability at construction, rechecks it, and checks both connected
socket peers against its authorized receiver address. There is no unauthenticated
service constructor or later `authorize` step. Authorization is checked on each
poll, before every outbound datagram, around incoming packet processing and at
poll completion. Detected revocation shuts down the clock and closes both sockets
immediately, including a failed direct send; restoring approval cannot resurrect
that owner. Failure to bind the standard ports is an error; PiAir never installs a
service, elevates itself, reuses another daemon, or changes host configuration.
These entry points have no production caller until PR #42. The lower-level
service accepts preconnected sockets only with the same mandatory capability.
Loopback fixtures explicitly create temporary allowlist/selection files and mock
authenticated control keys, then use the production capability-issuance and
revocation checks. Pure Grandmaster packet/state tests require no network lease. No receiver endpoint comes from a plist
peer list, DNS, a redirect, model, name or discovered group member.

The future session owns the authenticated Wire, service and media transport.
It must poll timing on its own bounded event loop, rather than blocking it during
control I/O. There are no timing threads. Drop releases the two timing sockets;
cancellation, authorization failure and fatal socket errors close them immediately.

The selected policy is a **fixed sender grandmaster** for a single approved
receiver. It sends Announce once per second and two-step Sync/Follow_Up at 8 Hz.
The follow-up carries the matching sequence and the monotonic observation taken
immediately after the actual Sync send. Delay_Req gets a Delay_Resp naming the
receipt time and requesting port identity. Pdelay_Req gets a two-step response
and matching Pdelay_Resp_Follow_Up with an observed send time. Software timestamps
include syscall/scheduling uncertainty; they are not NIC hardware timestamps.
Standard path-trace and 802.1AS Follow_Up information TLVs are emitted. Unknown
Apple organization-specific TLV semantics and signaling negotiations are **not
implemented**. Signaling is structurally decoded but is rejected by the fixed
role engine and cannot refresh timing liveness.

No BMCA takeover, receiver-as-grandmaster election or group topology expansion is
performed. Incoming Sync/Follow_Up/Announce cannot change the sender's timeline.
The grandmaster honestly advertises class 248, unknown accuracy, internal
oscillator and no UTC/GPS/traceability. The pinned fork advertises a stronger
quality; we deliberately do not claim a GPS clock. Whether strict firmware follows
this honest quality profile is a hardware-validation dependency, not an excuse
to report lock or invent a hardware workaround.

## State, evidence and recovery

Grandmaster state transitions are:

- `Initializing` -> `Synchronizing` when serviced.
- `Synchronizing` -> `PeerObserved` only after a valid delay request from the
  connected receiver with the expected clock ID supplied by authenticated setup.
- Missing initial evidence by the explicit setup deadline (maximum 30 seconds),
  three seconds without valid peer traffic, monotonic reversal, or a service
  gap greater than three seconds -> `Lost`.
- Cancellation or explicit shutdown -> `Shutdown`.

`PeerObserved` means traffic, **not receiver servo lock**. A grandmaster does not
measure the receiver's offset from delay requests, and cannot honestly report
its remote clock synchronized. The implementation does not promote it to
`Locked` after a timer, packet count, SETPEERS or RTSP 200. `media_permitted()`
is always false, in every state; the existing session gate also stays closed.
Packet failures do not refresh liveness or advance replay state. Sequences are
tracked separately for E2E/P2P requests with 16-bit serial arithmetic, accepting
wrap and rejecting duplicates/backward/half-range jumps. Source clock and port
identity are pinned; changes require a new session. UDP requests are bounded to
512 bytes, each poll handles at most 16, and malformed/unsupported requests are
ignored without poisoning an otherwise healthy service. Fatal socket/lease
errors shut it down. There is no unbounded retry queue or catch-up burst.

A lost grandmaster cannot silently resume/rebase. The future session must stop
media, tear down, and create a fresh clock/session identity and sample mapping.
Process-unique generation IDs additionally reject old mappings after owner
recreation; they are local validity tokens, not network clock identities.
PR #42 must ensure new identity/epoch negotiation across process restarts too.
A fresh service can synchronize again; old owners remain terminal.

The independent `Estimator` supports tests and a future explicitly selected
follower profile; it is not wired into role election. It consumes **correlated**
four-timestamp measurements (master t1/t4, local t2/t3), preserves signed
correction fields in units of 2^-16 ns, solves local-minus-master offset and
mean path delay, and estimates drift in integer ppb. It requires four distinct
valid exchanges spanning at least one second before `Locked`. Exchange spacing
is 1 ms..3 s, path delay 0..100 ms, drift <=500 ppm, offset discontinuity <=5 ms.
These bounds are conservative local policy, not Apple protocol constants.
Stale/reversed exchanges, excessive drift/steps and extrapolation beyond three
seconds invalidate it and its generation. Fresh evidence is required for
recovery. Symmetric-path bias and software timestamp jitter remain unmeasured;
this estimator is not a calibrated servo or a promise of Pi oscillator accuracy.
No system clock API is called to set or discipline time.

## Precision and timeline rules

PTP timestamp encoding is 48-bit seconds plus 32-bit nanoseconds, network byte
order. Nanoseconds must be below 1,000,000,000; seconds beyond the PTP era are
rejected instead of silently wrapping. Network anchors use unsigned Q64 seconds
fractions: `(nanoseconds << 64) / 1_000_000_000` in u128, retaining the low 32 bits
that the reference's conversion drops. Timestamp conversions round down by less
than 2^-64 seconds. Network time is the sender's private monotonic epoch, not
Unix time, NTP time, UTC, or the process wall clock. Deadlines use `Instant`
separately; deadlines are never compared with a remote timestamp.

`SampleTimeline` maps **absolute u64 samples** to an origin at exactly 48,000
samples per second. Each query computes a rational delta from that origin in
i128; it never accumulates floating-point frame durations. Negative deltas use
Euclidean division. Reverse mapping returns the nearest sample (ties later).
Large spans and overflows are checked. Only serialization wraps absolute samples
to the RTP u32 timeline. A discontinuity needs a new mapping/generation and a new
anchor acknowledgement; it never rewinds media encryption counters.

An anchor is generated only with fresh valid peer traffic, matching clock ID
and generation, and a full-precision time between now and ten seconds ahead.
It is checked again before control transmission so a held request cannot become
a stale schedule. Rate is an enum containing only pause 0 and play 1; flags are
zero. The anchor expresses a schedule on the local grandmaster timeline. It is
not proof that the receiver has adopted that schedule.

## Authenticated control and trust boundaries

`Wire::buffered_timing` requires control keys and current authorization.
Direct typed Control construction also requires Wire to issue a concrete PTP
capability and derives its peer addresses from that Wire; keys without persisted
or operation authorization cannot create network-capable timing control. Its
exclusive Wire borrow binds all operations to that authenticated connection and
receiver. SETPEERS is the RTSP **method**, uses the session URI, Content-Type
`/peer-list-changed`, and a bare binary-plist array ordered receiver address,
local control-socket address. Only those two addresses can be constructed by
this adapter. Group/proxy peers and arbitrary address lists are unsupported.

SETRATEANCHORTIME is the RTSP method on the same URI, Content-Type
`application/x-apple-binary-plist`. Fields are `networkTimeSecs`,
`networkTimeFrac`, `networkTimeTimelineID`, `networkTimeFlags`, `rtpTime`, `rate`.
Fraction and timeline ID use signed eight-byte bplist integers preserving their
unsigned bit patterns; plist unsigned values above i64::MAX can otherwise be
encoded in sixteen bytes. Timeline ID is the same sender clock ID negotiated
by the future authenticated PTP SETUP, not an NTP identity or receiver ID.

SETPEERS acknowledgement is required before an anchor. Existing Wire framing
validates authenticated encryption, bounded lengths, CSeq, RTSP 200, revocation
and control deadlines. A timing body may be empty or a bounded binary-plist
dictionary: `status` must be integer zero, optional anchor echoes must match
exactly, and unknown or contradictory fields are rejected. Malformed body,
nonzero in-band status, non-200 RTSP, bad CSeq, timeout or contradictory echo
closes the Wire with no automatic retry/fallback. Semantic validation is stricter
than the fork's permissive malformed-body behavior. Empty success is protocol
evidence of acceptance only, not remote lock or media permission.

PTP UDP remains unauthenticated. Address, clock/port IDs, sequence checks and
bounded state limit mistakes/replays but do not prevent on-path spoofing or
forged forward sequences. No cryptographic PTP authentication is claimed.
Authenticated timingPeerInfo binding and session-owned cancellation are required
before production activation. No timing payloads, decrypted control responses,
keys or samples are logged by the new code.

## Protocol audit and references

Sources were studied for protocol facts only. No GPL implementation was copied,
translated, linked, vendored or used as an implementation skeleton. This is not
a claim of personnel-separated clean-room development. New runtime code uses
only existing dependencies. Main's buffered, session, profile, wire, planner,
realtime timing, compatibility model and prior source ledgers were inspected.

| Source / immutable revision | Evidence used and limits |
| --- | --- |
| [lo-tech-systems/owntone-mini e9b058448b7d921ed048790b45614685fd13802b](https://github.com/lo-tech-systems/owntone-mini/tree/e9b058448b7d921ed048790b45614685fd13802b), `src/ptpd.c` | Service wrapper, peer add/remove/stats, private monotonic sender grandmaster timescale. Its reuse of an external daemon and truncated fraction are not adopted. |
| Same revision, `src/libairptp/src/ptp_definitions.h`, `ptp_msg_handle.c`, `airptp_internal.h` | Cross-check message numbers/layout, transportSpecific=1, domain=0, source port 0x8005, two-step Sync, Delay/Pdelay flow, role and TLV presence. Apple-specific extension behavior remains unresolved; no implementation code incorporated. |
| Same revision, `src/outputs/airplay.c`, timing-peer payload, SETPEERS, anchor payload and response handling | Exact methods, peer order, Content-Types, fields, signed fraction/clock bits, 0/1 rates, nonzero in-band rejection. Group expansion, model workarounds and claiming high clock quality are not adopted. |
| [Independent openairplay receiver 6c343d3679ddb561c61566985acaaf587d0a3bd3](https://github.com/openairplay/airplay2-receiver/blob/6c343d3679ddb561c61566985acaaf587d0a3bd3/ap2-receiver.py), `do_SETPEERS`, `do_SETRATEANCHORTIME` | Independently corroborates RTSP methods, bare address array, peer Content-Type, rate 0/1, rtpTime and empty RTSP 200. It does not validate a full Apple timing servo or all anchor fields. |
| IEEE 1588-2008, sections 13.3, 13.4, 13.5; IEEE 802.1AS-2011 Follow_Up information TLV | Common 34-byte header, ten-byte timestamp, signed scaled correction, message fields and organization TLV layout. Independent [1588 layout appendix](https://www.diva-portal.org/smash/get/diva2:859500/FULLTEXT01.pdf) and [PTP profile guide](https://www.kernelkit.org/infix/26.09/ptp/) cross-check offsets, domain/transport and role distinctions. Standards do not specify Apple's private signaling. |

The hand-assembled known-answer packet in `ptp/tests.rs` follows IEEE layout
tables, rather than round-tripping an encoder-generated fixture. Analytic timing
fixtures solve independently chosen delay/offset/drift values. Standard-library
Python plist fixtures in `ptp/fixtures/` independently check bplist wire fields.
The tests use only deterministic arithmetic and local TCP/UDP mocks, including
encrypted fragmented RTSP replies. No physical receiver is contacted.

## PR #42 and hardware requirements

PR #42 must implement and validate the complete **single authorized receiver**
lifecycle before opening the buffered media gate:

1. Integrate authenticated /info policy and PTP session SETUP, same-session
   timingPeerInfo/ClockID/Addresses validation, fresh sender clock identity/epoch,
   timing ports and event connection. Missing/unsupported receiver ClockID must
   fail closed; do not derive it from a device name or untrusted UDP.
2. Negotiate SETPEERS and poll timing without starvation during RTSP operations;
   verify required Apple signaling and organization TLVs using independent
   protocol evidence and authorized captures. Establish an honest readiness
   criterion beyond elapsed time or delay traffic, or explicitly report that
   remote lock is unavailable. Review standard port permissions on Raspberry Pi.
3. Implement required command/audio-mode/stream SETUP and RECORD ordering,
   type103 format/key/port validation, feedback/event handling, a fresh sample
   mapping, rate-anchor acceptance, and media authorization as one session owner.
   Neither this Control adapter nor its successful acknowledgement is that owner.
4. Obtain the buffered TCP socket through Wire authorization; connect only the
   original authorized address and authenticated negotiated data port. Enforce a
   revocable readiness grant immediately before every media write, plus bounded
   backpressure, cancellation and timing-loss shutdown. No discovered group
   member may be implicitly approved or connected.
5. Define pause/flush/resume discontinuities, new anchors and outstanding response
   matching, sequence/nonce continuity, bounded TEARDOWN, all partial-setup error
   paths and recovery with a fresh negotiated clock epoch.

Apple TV and standalone HomePod require separately authorized hardware trials:
record firmware/model, packet captures, accepted identity/domain/quality and
TLVs, Delay versus Pdelay traffic, peer port identity changes, actual receiver
clock readiness evidence, signed plist widths, rate/anchor acknowledgements,
48 kHz rollover/long-run drift, scheduling jitter on Pi under load, packet loss,
Wi-Fi asymmetry, idle timeout, suspend/discontinuity and recovery. Confirm timing
loss blocks media and teardown removes sockets. Physical audibility, latency and
channel/sample correctness require separate measurement, not RTSP status alone.
Proxy groups, stereo groups and Apple TV/HomePod grouped playback remain out of
scope; do not validate them by automatically authorizing additional members.


## Mandatory authorization regression coverage

The senior-review correction removes the optional-service-lease bypass without
changing legacy realtime NTP or authentication leases. Tests reject absent
approval even when mock control keys exist, approval without authentication,
closed Wires, revoked capabilities at attachment and sockets targeting another
receiver. Revocation during an owner's lifetime (allowlist removal, missing or
malformed selection, malformed config) prevents scheduled and response packets,
ignores queued timing input and immediately releases sockets. Per-datagram tests
revoke after a successful send and verify that Sync, Follow_Up, Delay_Resp,
Pdelay_Resp, Pdelay_Resp_Follow_Up and Announce cannot transmit afterward. Typed
timing control is also tested for revocation before its first request. All
network fixtures are explicit loopback authorizations; no physical receiver is
contacted, discovered or automatically approved.
