# Receiver capability and transport planning foundation

Current single-receiver status: [BUFFERED_LIFECYCLE.md](BUFFERED_LIFECYCLE.md)
implements an opt-in authenticated initial PTP SETUP diagnostic and bounded
cleanup. Live buffered playback remains closed because receiver clock adoption
is unverified. Historical PR #42 prerequisite lists below remain unresolved
except for that explicitly documented foundation. No verified audio is claimed.

Current foundation update: [BUFFERED_TRANSPORT.md](BUFFERED_TRANSPORT.md) supersedes
historical statements below that all buffered encoders/transport are absent.
ALAC24 and type103 TCP components are implemented; complete sessions and live
buffered playback remain disabled pending PR #42. The [PR #41 timing foundation](PTP_TIMING.md)
is implemented separately. AAC remains unavailable.


Audit baseline: PiAir `f24b447` (PR #38 merge), 2026-11-10. This is original
Apache-2.0 Rust policy/model code. GPL sources were inspected for protocol facts;
no GPL code was copied, translated, vendored, linked, or used as a code skeleton.

## Audit and source ledger

Before editing, inspected `src/discovery.rs`, `receiver/compatibility.rs`,
`receiver/profile.rs`, `receiver/mod.rs`, `receiver/session.rs`, CLI dispatch in
`src/cli.rs`/`main.rs`, `receiver/interactive.rs`, receiver unit/loopback tests,
`tests/cli.rs`, and architecture, pairing, session, buffered and reference docs.
Discovery already tracks updates/removals and merges only validated device IDs.
Selection pins endpoints, identity hints, capabilities and explicit auth method;
`wire.rs` remains the connection authorization boundary. Existing realtime
control/media implementations and their tests are unchanged.

References are immutable revisions, not product-version guesses:

| Source revision | Paths / observed facts | Limits of evidence |
| --- | --- | --- |
| [owntone-mini minimal e9b058448b7d921ed048790b45614685fd13802b](https://github.com/lo-tech-systems/owntone-mini/tree/e9b058448b7d921ed048790b45614685fd13802b) | `src/outputs/airplay.c:777` feature map; `:5730` buffered format parsing accepts extended format-ID arrays and supportedFormats masks; `:5814` info handler gates formats; `:7585` reads gid/gpn/tsid/gcgl/igl | Claims and source behavior, not receiver conformance or hardware verification. Both format encodings must agree in PiAir; unknown IDs remain evidence. |
| Same fork/revision | `src/outputs/airplay.c:7053` normalizes HomePod tsid and +0/+1 gids using model classification; `src/outputs.c:439` refreshes groups and `:509` considers Apple TV proxy leadership via matching raw gid/igl; `src/player.c:2853` expands stereo selection | PiAir keeps separate gid/tsid claims, never identifies device classes from names/models, elects leaders, hides members, or expands selections. Matching explicit IDs creates candidate relationships only. |
| Same fork/revision | `src/outputs.c:898` resolves auto transport; `airplay.c:7637` schedules deferred capability probes; `src/player.c:2486` completes proxy groups during playback | PiAir deliberately has no auto mode, network probe, cached capability promotion, or group connection behavior. Fork's "passive_probe" still makes a connection; it is not passive discovery by PiAir's definition. |
| Same fork/revision | `src/outputs/airplay_buffered.c:45` type103 framing; `:388` encrypted queued writes; `airplay.c:4661` PTP group offer; `:7961` anchor gate; `src/player.c:2688` flush lifecycle | New fork has actual buffered transport and anchor/lifecycle machinery. None is enabled or ported here. Physical audibility, firmware coverage, timing lock and failure recovery remain unverified in this audit. |
| [historical owntone-server f7ace58dc2aeb5305360770da37fd62095f5bb21](https://github.com/owntone/owntone-server/tree/f7ace58dc2aeb5305360770da37fd62095f5bb21) | `src/outputs/airplay.c:88` type96; `:2633` realtime stream offer; `:2719` separate PTP offer; `:4070` PTP capability/config choice. No `src/outputs/airplay_buffered.c` in this tree | Older PiAir research correctly describes this revision's realtime path. Its "no active buffered sender" conclusion must not be generalized to the newer fork. Bit41 alone does not prohibit NTP. |
| Same historical revision | `src/outputs.c` output lifecycle and `src/player.c:2107` starts selected outputs; `:1891` flushes outputs | No equivalent newer candidate/proxy grouping layer observed in historical outputs.c. Historical research is retained as history, not replaced with fork assumptions. |

The earlier [source analysis](AIRPLAY_REFERENCE_ANALYSIS.md) and
[buffered investigation](BUFFERED_RESEARCH.md) retain their original pinned
revisions. This ledger supersedes any reading that applies their historical
buffered limitations to all OwnTone forks.

## Model and policy boundaries

`receiver/planning.rs` separates `Evidence` (mDNS service provenance or explicitly
authenticated /info provenance), typed `Claim` values, local `Implementation`, and
`RuntimeResults`. Unknown, invalid and conflicting values remain distinct.
Raw evidence is restricted to bounded recognized non-secret protocol fields;
unknown TXT, public keys and entire /info payloads are never retained or printed.
The info parser projects only features, status and format scalars/IDs. It accepts
an already decoded value; the session's bounded binary-plist parser must precede
it in any future live integration. Calling the projection does not authenticate
anything. A future caller must bind it to the same authenticated receiver and
session; no new cache, credential read or /info request is introduced here.

AirPlay audio bit9 is a prerequisite claim for a realtime candidate, not a
realtime type96 acceptance guarantee. ALAC bit19 is distinct from exact sample
format. RAOP service existence is a claim for AirPlay 1, not proof of its codec,
authentication or timing acceptance. Local realtime support is specifically
ALAC/44100/16/stereo, NTP, explicit authentication and bounded diagnostics.
Missing sample-format metadata leaves acceptance unknown until the existing
explicit authenticated session validates it. Malformed or conflicting formats
reject planning. Advertised PTP is separate from a caller's explicit PTP-only
requirement; the latter rejects NTP with no fallback.

### Transport independence (senior-review correction)

The planner evaluates each media profile independently. Only authorization and
topology restrictions apply to every candidate. AirPlay 2 profiles additionally
share the exact AirPlay endpoint and explicit HAP/authentication policy; RAOP does
not consume that policy or AirPlay 2 `/info` status. Discovery evidence is retained
per service, but RAOP feature/sample claims never become AirPlay 2 requirements.

| Profile | Capability and format evidence | Local status |
| --- | --- | --- |
| Realtime NTP/PTP | AirPlay audio bit9, ALAC bit19, AirPlay-service sample claims and authenticated `audioStream` format 0x40000; NTP rejects a PTP-only requirement, PTP requires bit41 | Only bounded NTP diagnostics are implemented. Both realtime timing variants check the same realtime codec/sample format. |
| Buffered PTP | Buffered bit40, PTP bit41 and authenticated `bufferStream` format IDs; no dependency on realtime bit9, bit19, `audioStream`, or realtime sample claims | Transport and all buffered codec variants remain unavailable for streaming. |
| RAOP | Its own RAOP service and exact endpoint identity; codec/encryption/transport/authentication dialect requirements remain explicitly unresolved | Protocol profile unavailable; no HAP or AirPlay 2 format prerequisite is borrowed. |

Feature reconciliation compares only bits required by the profile, rather than
requiring entire feature words to match. For example, a buffered-bit change cannot
invalidate realtime ALAC, and an ALAC-bit change cannot invalidate a buffered AAC
claim. Authentication features are still checked using the existing strict policy;
format independence cannot relax authorization or authentication restrictions.

Buffered media diagnostics expose typed format variants: ID22 AAC-LC 44.1 kHz
stereo, ID23 AAC-LC 48 kHz stereo, ID21 ALAC 48 kHz/24-bit stereo, and ID39 AAC-LC
48 kHz surround. These mappings are facts from the pinned fork's
`src/outputs/airplay.c:117-135`, not baseline codec assumptions. Other IDs retain
`Unknown(id)` and unknown codec availability. Recognized variants have explicit
unavailable local codec status: PiAir has no AAC encoder or matching buffered
ALAC24 encoder. Its realtime ALAC encoder does not supply either implementation.

Bit40 alone leaves buffered formats **unknown**. Only an explicitly authenticated
`/info` projection can provide format candidates; an absent list stays unknown,
an empty list supplies no format, and malformed/conflicting mask/list evidence
rejects evaluation. Known-but-unimplemented and unknown variants each have explicit
codec/format rejection reasons. No buffered candidate is eligible for streaming,
including receivers advertising AAC-LC without realtime ALAC. Authenticated format
observations are compatibility evidence, never proof of usable local codecs or
physical playback.

Plans have stable preference order (realtime NTP, realtime PTP, buffered PTP,
RAOP), with eligible candidates first and all rejection reasons retained. Eligibility
is advice for an explicit diagnostic, not permission to connect, enroll, downgrade,
fall back or play. Authentication uses the existing strict compatibility policy.
No method is chosen by default. Buffered/PTP/RAOP implementation status is always
unsupported for actual streaming, even with positive capability evidence.
The existing pairing, allowlist, selected receiver and streaming CLI paths do not
consume this planner and retain their authorization checks.

Topology is a read-only current-snapshot projection. Each claim keeps the service
and receiver identity. Only explicit validated group IDs (`gid`, `tsid`) create
candidate sets. UUID +0/+1 suffixes are retained, not normalized into membership;
`igl` and `gcgl` are hints, not elections. Matching gid with a leader hint can
show a proxy candidate (including Apple TV/HomePod deployments); matching tsid
can show a stereo candidate. Neither assigns a product class or proves roles.
Separate discovery, matching names, gpn labels, model strings, IPs and hostnames
do not correlate identities or membership. Multiple IDs or leader hints are
ambiguous. Malformed IDs are retained as invalid; unsupported ID syntax remains
unresolved. No full physical topology can be recovered from missing metadata.

Snapshots are rebuilt after discovery's service-name replacement and
ServiceRemoved/expiry handling. Removed or changed advertisements contribute no
old membership to a later snapshot; there is no persistent topology cache.
Advertisements not yet expired can still be stale or spoofed. No wall-clock
freshness or membership truth is claimed, and plans must be recomputed after
changes. Group claims conservatively block planning until group handling exists,
even when only one currently visible member advertises a gid.

## Passive CLI

`piair receiver-plan [--method pin-display|setup-code|transient]` is additive.
It browses mDNS exactly as `devices` does, then formats the pure model and plan.
It never loads credential/selection storage, opens receiver sockets, fetches
/info, changes volume, saves state or plays audio. Diagnostic authenticated-info
and runtime fields therefore show unknown values. `--method` expresses planning
intent only and does not authorize enrollment. IDs are generic, never hardcoded.
Output uses escaped Debug strings for untrusted values to avoid terminal control
injection; no sensitive payload or pairing material is rendered.

## Traceable gap matrix and follow-up boundaries

| Area | Foundation here | Required subsequent work / validation |
| --- | --- | --- |
| Buffered transport | bit40 claim, exact /info format IDs/masks, permanently unsupported profile | Independently implement type103 negotiation, codec mapping, encrypted bounded TCP queue/backpressure, nonce lifetime and cleanup. Trace fork airplay.c / airplay_buffered.c above; validate independently on hardware. |
| PTP and anchors | bit41 claim, explicit PTP-only rejection, no clock service | Separate clock ownership, lock/identity/domain/peer lifecycle, anchors and accepted rates; no fabricated NTP-to-PTP identity. Validate firmware and on-wire lock/anchor behavior. |
| Playback lifecycle | runtime negotiation/media/audibility remain unknown in passive model | Session-owned fresh evidence/results, cancellation, feedback, pause/flush/restart and capture integration; retain existing wire authorization boundary. |
| Groups/proxies/stereo | provenance-bearing raw hints, candidate sets, ambiguity and snapshot replacement | Authenticated membership evidence, member-specific authorization, leader loss/re-election policy, proxy lifecycle and coherent teardown; never auto-authorize followers. Validate standalone ATV, ATV/HomePod proxy and stereo transitions physically. |
| RAOP | AirPlay 1 service claim and unsupported profile | Separate ANNOUNCE/authentication/encryption/codec/timing design and implementation; never substitute it as an authentication downgrade. |
| Capability cache | none | Explicit same-identity/session binding, expiry, revocation, contradictions and sanitization before any persistence. |

Advertised buffered support and RTSP success do not prove audible playback.
Even authenticated /info format support and media-send success are insufficient.
Hardware validation must separately establish audible output, channel mapping,
synchronization, startup/stop behavior and recovery on each tested firmware and
access policy. Synthetic fixtures prove conservative policy, not hardware support.


### PR #41 timing capability update

[PTP timing and control](PTP_TIMING.md) are independently testable foundations.
BufferedPtp remains FoundationOnly and ineligible for live playback. RealtimePtp
is not activated; realtime NTP stays unchanged. Sender-grandmaster peer traffic
is not a verified receiver servo lock, and no RuntimeResults hardware success is
recorded. Complete single-receiver negotiation, feedback, revocable media grants,
Apple extension validation and hardware trials remain PR #42 dependencies.
