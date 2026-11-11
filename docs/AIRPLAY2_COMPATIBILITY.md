# Post-PR #49 clock adoption investigation

See [the focused investigation](CLOCK_ADOPTION_INVESTIGATION.md) for the pinned
reference comparison, identity/role uncertainties, diagnostic changes and safe
Stua-only staged verification. Receiver adoption remains unverified. In the
historical ledger below, “source clock” describes the reference builders; the
universal semantics of receiver-generated Apple subtype4 remain unknown.

# Single-receiver protocol progression (PR #43)

Base: main `28e5806` (PR #42 merged). PRs #39–42 supplied capability planning,
type103/ALAC24/encryption/TCP primitives, PTP and the initial session diagnostic.
This revision advances **control diagnostics**, not playable operation. All
verification is arithmetic, encrypted loopback mocks and CLI tests. No physical
receiver was contacted; no audible playback or receiver clock lock is claimed.
No OwnTone, Autostream or system service changes are involved.

## Evidence ledger

OwnTone-mini is pinned to **e9b058448b7d921ed048790b45614685fd13802b**.
All OwnTone links below use that revision. Its GPL AirPlay/session files were
read solely for observable protocol facts: no code copied, translated, vendored,
linked or used as a code skeleton. The PTP file at this revision carries MIT;
it was likewise used only as a reference. This is not a personnel-separated
clean-room claim. No new runtime dependency or reference source enters the build.

Independent corroboration:

- [openairplay receiver, 6c343d3679ddb561c61566985acaaf587d0a3bd3](https://github.com/openairplay/airplay2-receiver/blob/6c343d3679ddb561c61566985acaaf587d0a3bd3/ap2-receiver.py): initial/stream SETUP, timing addresses, RECORD, bare SETPEERS, anchor rates. GPL reference, no incorporation. A receiver implementation cannot establish Apple firmware behavior.
- [Music Assistant airplay-cli, 0a58c239d5fb3cf8927b9707a71c6fe897266b6a, ap2_ptp.c](https://github.com/music-assistant/airplay-cli/blob/0a58c239d5fb3cf8927b9707a71c6fe897266b6a/src/ap2_ptp.c#L475): independently corroborates the Apple Follow_Up extension's OUI, subtype, length and network-order clock ID, plus empirical signaling subtypes. Apache-2.0. This is implementation corroboration, not a normative Apple specification or proof of independent capture provenance.
- [cidermill-airplay 0.1.5 command documentation](https://docs.rs/cidermill-airplay/0.1.5/cidermill_airplay/command/index.html): reported iPhone capture corroborates `/command`, `updateMRSupportedCommands`, `params` and nested command-info blobs. Documentation only; no code incorporated. It uses SETPEERSX and does not prove the fork's empty-then-full ordering is mandatory.
- IEEE 1588-2008 sections 13.3–13.5 and 16.1; IEEE 802.1AS-2011 Follow_Up information TLV: 34-byte header, 48-bit seconds/32-bit nanoseconds, flags, serials, identities, organization TLV framing. See the independent layout references in [PTP_TIMING.md](PTP_TIMING.md). These do **not** define Apple's private TLV semantics, RTSP startup or receiver adoption.

Status: I = implemented; M = missing/deferred; U = uncertain. Required means a
condition for PiAir's diagnostic, not an assertion that every receiver requires it.

| Behavior and exact OwnTone location | Before (#39–42) | This revision | Classification / evidence limit |
| --- | --- | --- | --- |
| [initial PTP SETUP, L4623](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/outputs/airplay.c#L4623) | I: timingPeerInfo, signed ClockID, Addresses, timingPeerList, fresh UUID | I, unchanged | Authenticated peer/address binding required. Fork also sends macAddress/groupUUID/clock-port override; necessity U, omitted rather than invented. No group/proxy connections. |
| [handle_timingpeerinfo, L5294](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/outputs/airplay.c#L5294) | I: exact authorized address, mandatory ClockID | I: same conservative policy, missing-ID rejection tested | Fork registers first usable address **without requiring ClockID**. Missing ID is valid protocol behavior, but secure discovery of its expected identity remains M; see below. Multiple-address fallback is not adopted. |
| [sequence table, L6592](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/outputs/airplay.c#L6592) | M beyond initial SETUP | I: initial SETUP → RECORD → SETPEERS → command → stream SETUP → audioMode → optional paused anchor | Same authenticated Wire, token and receiver throughout. Full playable sequence M. No volume operation. |
| [SETPEERS, L4549](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/outputs/airplay.c#L4549) | I component only | I integrated; bare array receiver then local, `/peer-list-changed` | Independent receiver corroborates shape. Receiver-self-first necessity is empirical. Additional group peers excluded. |
| [command builders, L3593/L3629](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/outputs/airplay.c#L3593) | M | I empty supported-command registration; M full enabled table | Shape corroborated independently. PiAir handles no MediaRemote commands, so it advertises none. Fork's broad enabled command list/second POST is a receiver workaround, not proven mandatory; do not falsely promise remote controls. |
| [stream SETUP, L4400](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/outputs/airplay.c#L4400) | I offer/parser components, M owner integration | I integrated: type103, format21/bit21, ct2, spf352, fresh 32-byte shk and signed random 64-bit streamConnectionID | Reuses PR #40 Prepared; no key import/reuse. streamConnections key-use true. Fork's clientID=com.apple.Music omitted: PiAir is not Music; receiver requirement U. |
| [stream response, L5120](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/outputs/airplay.c#L5120) | I flat dataPort parser | I nested RTP streamConnectionKeyPort and flat dataPort; reject disagreement, invalid ports, nonzero status, key-policy changes and format/ID conflicts | Required response validation. No media socket is opened, even with a valid port. TCP failure/counter behavior remains covered by PR #40 component tests. |
| [audioMode, L3690](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/outputs/airplay.c#L3690) | M | I POST {audioMode:default} after stream SETUP | Empirical receiver expectation, not a published conformance requirement. Treat as diagnostic negotiation; physical acceptance U. |
| [anchor builder, L3754; response, L5615](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/outputs/airplay.c#L3754) | I generation/ack component, M owner integration | I one fresh paused anchor, exact echo/status validation, matching clock generation, freshness before/after request | Independent receiver corroborates rate/rtpTime. A 200 with nonzero status fails. Empty 200 is acceptance only; malformed bodies fail. No retries, rate=1 or media activation. |
| [PTP constructors / TLV handlers](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/libairptp/src/ptp_msg_handle.c) | I standard Announce/Sync/Follow_Up/Delay; M Apple extension/general handling | I Apple Follow_Up clock TLV, strict recognized Apple lengths, duplicate/conflict rejection, bounded general-channel drain | Independent wire corroboration above. Private signaling subtypes 1/5 decoded as opaque; emitting their unexplained marker and unicast grant negotiation M. No peer-add/delete TLV can create sockets or peers. |
| Same PTP file: Announce quality / fixed GM assumption | I truthful class248/unknown/internal oscillator, no BMCA | I unchanged; independent Announce sequence now fixed | Fork deliberately uses class6/accuracy0x21/GPS. This receiver workaround would falsify PiAir's quality. Strict-receiver adoption U. Full BMCA/follower role M. |
| [ptpd.c lifecycle](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/ptpd.c) | I session-owned connected authorized sockets | I scoped servicing during pending control incl. healthy TEARDOWN | Fork may reuse a host daemon and registers addresses. PiAir owns sockets, does not reuse daemons, and rechecks leases per datagram. |
| Playback permission / media | M, hard closed | M, hard closed | Peer activity, local estimator lock, accepted anchor and elapsed time do not attest remote servo lock. No defensible adoption signal has been established. |

## Clock identity, quality and readiness

Missing ClockID is **not** a malformed OwnTone-compatible response. However, the
stated security requirement is stronger than address-only peer registration.
PTP v2 UDP sourcePortIdentity, Announce and Apple's repeating clock TLV are
unauthenticated, attacker-selected values. A first packet from an authorized IP
cannot cryptographically bind a previously unknown ClockID to authenticated
control, and multiple apparently agreeing messages do not repair that. No
independently corroborated authenticated challenge/response or signed identity
binding was found in the studied locations. Therefore PR #42's missing-ClockID
blocker remains deliberately closed. No device-ID conversion, fabricated clock
or trust-on-first-UDP-packet fallback exists. With a control-supplied ClockID,
PiAir pins the full source port identity from valid delay requests, rejects
changes/replays/reflections and never lets general traffic establish the pin.
This still cannot defeat an on-path attacker; no authenticated PTP is claimed.

Announce remains priority1/2=128, class248, accuracy0xfe (unknown), variance0xffff,
timeSource0xa0 (internal oscillator), UTC offset zero with UTC-valid and
traceability flags clear. The local epoch is arbitrary monotonic time, so PiAir
also keeps ptpTimescale clear rather than implying TAI. Consequently its Sync
flags are 0x0600, other generated timing flags 0x0400; the fork uses 0x0608/0x0408.
If strict firmware requires TAI/high-quality claims to follow, that is a remaining
compatibility limitation. Changing priorities cannot prove clock adoption.
There is no BMCA implementation; incoming Announce/Sync/Follow_Up do not elect
another GM or change PiAir's clock. The service is a fixed single-peer diagnostic.

Apple subtype4 Follow_Up extension: TLV type3, length16 (excluding four-byte
TLV header), OUI 00:0d:93, subtype 00:00:04, big-endian 8-byte source clock,
two zero bytes. Follow_Up is now 96 bytes including the 32-byte standard
802.1AS information TLV. Parser validates identity agreement, length and padding,
and rejects duplicate Apple subtypes. Signaling's ten-byte body is a target
port identity, not a timestamp; known Apple subtypes1/5 require lengths22/32.
They have no readiness meaning in PiAir. Targets must be local, IEEE wildcard,
or the empirically observed zero wildcard; unsupported targets cannot refresh
liveness. Unknown general messages and OwnTone-private peer TLVs have no effect.

Readiness has three separate facts:

1. PeerObserved: fresh, authorized-address, expected-ID, forward-sequence
   Delay_Req/Pdelay_Req activity. Indicates reachability only.
2. Receiver clock adoption: **unproven**. Neither a PTP request nor the local
   follower estimator can measure a remote receiver's servo as sender GM.
3. Anchor accepted: same-session RTSP 200 plus validated empty/zero-status or
   matching-echo body for a fresh paused schedule. Indicates control acceptance.

Only (1) and (3) can currently be observed. `media_permitted()` stays false,
Playing/Prebuffering/Paused remain unreachable, and no live TCP media connection
or rate=1 anchor is made. The remaining blocker is an independently justified,
fresh, receiver-bound indication of adopted GM/timeline and usable mapping,
including discontinuity and loss behavior; even that must not be equated with
physical audibility. Future media integration must recheck all authorization,
negotiation, adoption and anchor gates before every write, preserve PR #40's
nonce/sample progression and bounded queue, and continuously service timing.

## Ownership, bounds and failure behavior

Stages are single-use and ordered on the same owner. A valid pinned initial
Session token is mandatory to progress; changed or parameterized tokens fail.
All control responses pass existing AEAD/framing/CSeq/RTSP200/deadline checks;
nonempty acknowledgement bodies require supported plist content type when
present, valid dictionary semantics, zero status and no unknown/conflicting
fields. Required SETUP bodies must have the binary-plist Content-Type.

During each blocking control request a scoped worker exclusively borrows the
PTP service, polls every ~2ms and joins before any stage is accepted. No detached
worker can survive the call. Existing Wire cancellation/authorization checks
remain in place. PTP failure is propagated after the bounded request completes;
no subsequent stage advances. Every event/general poll handles at most16
packets per channel, <=512 bytes, and checks authorization before each datagram
send/receive handling. General traffic never extends the three-second delay
activity timeout. Initial evidence deadline is30s (maximum existing bound);
observation remains <=1s and control requests <=3s. Healthy PTP continues through
TEARDOWN; cancelled/lost timing is stopped before cleanup. Cleanup retains the
250ms request budget and destroys key/stream/event/control owners on all exits.
Remote expiry is explicitly unconfirmed when revocation or failed control makes
a TEARDOWN acknowledgement impossible. No bypass to send teardown after revoked
authorization is added.

## Safe staged hardware plan (manual only, not executed)

Do not run this plan automatically. Use one standalone already-approved receiver;
never a stereo/group/proxy topology. Do not change OwnTone, Autostream, services
or receiver volume. Read help locally first. Default `buffered-test device:<id>`
and `--confirm-audio` fail before discovery/connection; they cannot override the
audio gate. Explicit diagnostics require the existing selection/authentication
binding, current allowlist, a terminal and confirmation. Credentials must be
provisioned separately by an explicitly authorized workflow.

1. `piair --config <config> buffered-test device:<id> --setup-check`: confirm
   initial SETUP, observe <=1s PTP and TEARDOWN. Missing ClockID stops here.
2. `... --control-check record`: separately confirms initial SETUP and RECORD.
3. `... --control-check stream`: separately confirms initial SETUP, RECORD, then
   SETPEERS/empty MR table/stream SETUP/audioMode. Offers a fresh secret key but
   makes **no media connection**.
4. `... --control-check anchor`: additionally confirms a **paused** anchor.
   Requires fresh delay activity and an unexpired anchor. Acceptance does not
   attest clock adoption. No rate=1, audio or volume operation is performed.

All confirmations are collected before connection/timing starts, so human input
cannot starve PTP. Each invocation is a fresh session and bounded teardown.
Ctrl+C cancels. Port319/320 bind errors are fatal; do not alter services or grant
privileges automatically. Stop on identity/address conflicts. Record only
model/firmware, selected stage, sanitized error, observed delay activity and
TEARDOWN outcome; redact keys, decrypted payloads, samples and personal IDs from
captures. Authorized captures should examine GM quality/selection, both UDP
ports, signaling, source identity continuity and anchor in-band rejection. A
future separate playback test needs defensible adoption evidence and explicit
audio confirmation; this revision provides no way to run one.

## Test scope and roadmap

New tests cover a literal 96-byte Follow_Up known answer, Apple TLV length,
padding/identity/duplicate rejection, opaque signaling, truthful quality,
independent Announce sequences, nested/flat port conflict and key-policy/status
validation, encrypted complete diagnostic order and payloads, per-stage in-band
rejections, paused anchor acceptance, PTP continuation during a delayed RECORD,
CLI opt-in conflicts, resource/key release and no media connection even after
anchor acceptance. Existing missing-ID, identity pinning, replay, timing loss,
cancel/revoke, RTSP framing, TCP faults, ALAC24 and nonce tests remain required.
Mocks validate behavior, not Apple interoperability or audible output.

Remaining work, in order:

- Independently establish authenticated receiver clock binding when ClockID is
  absent. Keep rejected until then; address-only UDP pinning is insufficient.
- Resolve private signaling/unicast negotiations, honest-quality adoption and
  an observable continuously valid receiver adoption criterion. Physical trials
  require separate authorization and cannot manufacture protocol evidence.
- Implement any necessary full command table together with the actual supported
  command handling. Confirm audioMode/clientID/clock-port-override requirements
  without spoofing identity or unsupported capabilities.
- Only after these gates: integrate authorized TCP media, PR #40 ALAC24 and
  encryption, feedback/backpressure, rate=1 anchor renewal, pause/flush/resume,
  and revocable per-write grants, with failure injection and physical validation.
- Group/stereo/multiroom remain outside PR #43 and blocked on the above.

Validation on the final code tree (Rust 1.99.1, Linux x86_64):

- `cargo fmt --check`: passed.
- `cargo clippy --all-targets -- -D warnings`: passed.
- `cargo test --locked`: passed, **270 unit + 23 CLI tests**, zero failures.
- `git diff --check`: passed; Cargo.lock unchanged.

Rust tools and ALSA development files were provisioned under `/workspace`, not
added to this repository. The socket-denying command sandbox required running
the loopback suite outside that sandbox. An initial run exposed two test issues:
the delayed-RECORD fixture's 200ms budget was too small and the old malformed
organization-TLV fixture now failed at encoding. The mock budget is500ms and
the malformed receive fixture now injects its invalid bytes directly; the final
full suite above passed. None of these checks contacted physical receivers.

The focused [HomePod Delay_Req compatibility note](PTP_DELAY_REQ_COMPATIBILITY.md)
records the reported Stua zero-control header, parser regression, pinned
reference comparison and setup-only manual verification procedure.
