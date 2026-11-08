# Opt-in OwnTone RECORD-only diagnostic

This is an explicit diagnostic for the already approved, selected Even-Olai
HomePod **`device:3ae5c9a302ad` only**. It performs transient authentication,
encrypted initial NTP SETUP, event-channel connection, empty RECORD, and bounded
TEARDOWN. **RECORD 200 does not prove stream negotiation or audible playback.**
No physical receiver test is authorized for Codex; none was performed here.

The task supplies physical evidence from PR #34 of successful OwnTone transient
authentication, encrypted initial NTP SETUP, an active timing responder and
acknowledged TEARDOWN. This implementation builds on that evidence; it does not
independently repeat or extend it. The historical PR #33 TEARDOWN timeout does
not establish its cause. RECORD acceptance on this receiver is still unverified.

## Pinned source analysis (before implementation)

PiAir baseline is fresh `origin/main`
`1bfcaeaf478b449ed8487a762487ff75d28165bc`, the merge of PR #34
(`7beb8383f16102662d7c4a3702c77f89162a5b0a`). GitHub reports PR #34 merged.
Upstream OwnTone is pinned to
[`f7ace58dc2aeb5305360770da37fd62095f5bb21`](https://github.com/owntone/owntone-server/tree/f7ace58dc2aeb5305360770da37fd62095f5bb21).
The following are source observations, not evidence of HomePod acceptance.
Protocol behavior was implemented independently; no GPL source was copied.

| Area | Verified upstream behavior | PiAir comparison and diagnostic decision |
| --- | --- | --- |
| RECORD payload | [`airplay.c:2654`](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L2654): builder adds no bytes or headers. | Send an empty body, explicit `Content-Length: 0`, no Content-Type, Range or RTP-Info. No plist RECORD payload or speculative alternatives. |
| Sequence | [Sequence table L3640](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L3640): initial SETUP, RECORD, SETPEERS, stream SETUP, volume. SETPEERS builder skips NTP sessions (L2660). | Reuse initial NTP offer and stop after RECORD. Never invoke SETPEERS, stream SETUP, SET_PARAMETER, feedback, FLUSH or media transmission. |
| URI and common headers | [`request_headers_add`, L889](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L889), [`sequence_continue`, L3770](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L3770): session URL, CSeq, User-Agent, client name, Client-Instance, DACP-ID, Active-Remote; evrtsp adds length. No RECORD-specific header. Optional password/compile-time stream ID paths exist. | Keep the existing `rtsp://<local IPv4>/<random u32>` URI from SETUP and the existing OwnTone-profile envelope: `AirPlay/999.0.0`, PiAir client name, stable sender identifiers, `X-Apple-ProtocolVersion: 1`, length and CSeq. Preserve any validated RTSP Session token. No password, stream ID or fallback path added. |
| RECORD response | [`response_handler_record`, L3031](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L3031): only advances state. [Sequence callback L3708](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L3708) rejects missing/non-200 RECORD responses, explicitly skips CSeq comparison. No mandatory Audio-Latency or response payload validation. | Require encrypted RTSP/1.0 200, matching CSeq, bounded valid framing, no duplicate headers, transfer encoding, redirect, unsolicited decrypted frame data, connection-close directive or body. An absent response length means zero per existing RTSP parser. Session token/timeout checks remain. Optional Audio-Latency must be unsigned decimal u32; absent is valid and its value never changes behavior. This body/header strictness is PiAir policy, not a verified receiver requirement. |
| Event connection | [Initial SETUP handler L3180](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L3180) requires eventPort and calls `airplay_events_listen` before RECORD. Upstream warns but proceeds if connect fails. [`airplay_events.c:525`](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay_events.c#L525) opens TCP to the receiver; `client_add` (L134) creates a HomeKit events cipher. No outgoing handshake or wait for a first event. | Reuse `Wire::event` and the existing event keys/worker. Pin the control-peer IP and validated eventPort, reauthorize before/after connect, require success, and check worker health before/after RECORD. Idle events are permitted. Strict encrypted event parsing/ACKs remain; remote commands cannot start playback. |
| Event lifetime | [`airplay_events.c:447`](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay_events.c#L447): EOF/read/decrypt/respond failure removes a client. Global deinit drains remaining clients (L572). No per-RECORD close handshake. | Session owns event worker through TEARDOWN; local release shuts down its socket and joins it. No wait for unsolicited events, reconnect or retry. |
| Timing | [`airplay_init`, L4318](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L4318) starts global timing service. [`timing_svc_cb`, L2262](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L2262) replies to 32-byte `80 d2` queries with `80 d3`, source endpoint, timestamp and byte-2 echo. | Preserve PR #34's one per-session socket/early ready worker through TEARDOWN; existing full sequence echo and timestamps, pinned peer-IP filter, authorization lease before replies. No query is required for RECORD success. PTP remains unsupported. |
| Cleanup | [TEARDOWN builder L2423](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L2423) uses empty binary dictionary. Normal/failure handlers L3259 and sequence table differ in acceptance of non-200 cleanup. | Keep existing empty binary dictionary and Session token, same encrypted control connection and counters. Require 200, matching CSeq and empty response. Failure cleanup has a 250ms budget; successful-path requests retain the existing 3s read cap. Invalidated transport or revoked approval prohibits further sends. Local release always runs. |

Reviewed PiAir `session.rs` (authentication, negotiation and cleanup), `wire.rs`
(authorization, cipher/framing, CSeq, event/NTP workers), `profile.rs` (four-field
NTP offer, bounded plist/capabilities/ports), `realtime.rs` (timing reply, encoder,
latency logging), CLI, main dispatch, stream/worker/CLI tests and prior OwnTone
and session docs. Normal playback and legacy setup profiles retain their paths.
The record diagnostic does not initialize an encoder, generate audio packets or
connect the locally bound data/control UDP sockets to a receiver.

## Exact PiAir contract

The only invocation is:

```sh
piair stream-test device:3ae5c9a302ad --record-check --experimental-owntone-transient
```

It requires existing allowlist approval, explicit transient selection, current
identity/capability/endpoint pinning and interactive `y`/`yes`. Exact ID is checked
before discovery or contact. Missing ID/flag, `--setup-check`, `--setup-profile`,
`--setup-timeout-seconds`, explicit `--seconds` or `--volume-db`, and other
experimental profiles are rejected. The interactive chooser cannot select it.
Defaults and existing credentials, including Stua Apple TV credentials, remain
unchanged. No persistent pairing, enrollment, credential replacement or receiver
fallback occurs. The existing public sender identity can be initialized locally.

One control TCP connection and uninterrupted directional crypto counters carry:

1. Plaintext GET `/info`, CSeq 1, existing OwnTone policy/preamble validation.
2. Transient M1/M2, CSeq 2; M3/M4, CSeq 3. Verify M4 before installing keys.
3. Encrypted GET `/info`, CSeq 4; validate audio/ALAC and NTP capability policy.
4. Bind existing local sockets; start/confirm the NTP worker before initial SETUP.
5. Encrypted initial SETUP, CSeq 5, unchanged binary dictionary: deviceID,
   sessionUUID, timingPort, timingProtocol=NTP. Validate binary-plist type/body,
   NTP if supplied, nonzero u16 eventPort and optional nonzero u16 timingPort.
6. Connect a separate events TCP socket to the same pinned receiver IP and
   eventPort. Reuse independently derived directional event keys and existing
   worker. No control CSeq is consumed; event request CSeq is independently echoed.
7. Encrypted `RECORD <same session URI> RTSP/1.0`, CSeq 6, empty body and only
   existing common headers plus optional validated Session. Strictly validate reply.
8. Encrypted TEARDOWN, CSeq 7 on the complete sequence; retain NTP and event
   resources through the exchange, then close sockets/zeroize keys/join workers.

RECORD success is retained and logged independently of TEARDOWN. Cleanup failure
still fails the command; a prior RECORD success is never promoted to playback
readiness. Cancellation fails the command. A cancellation during an in-flight
response invalidates control, so cleanup can be skipped; a usable authorized
connection permits bounded TEARDOWN despite cancellation. Revoked authorization
prohibits further contact, including timing replies and TEARDOWN. Encrypted
response waits poll at 100ms with a fixed deadline, without restarting a request.
Authentication/event-connect waits retain existing bounded socket operations.
No automatic retry or alternative protocol is attempted.

Expected sanitized log phases: OwnTone GET and SRP proof, encrypted GET, `NTP`
`started`/ready, `initial SETUP` validated, `event connection` connected, `RECORD`
attempted then validated or rejected/timed_out/connection_closed/failed,
`TEARDOWN` attempted then acknowledged/rejected/timed_out/connection_closed/failed
or skipped, NTP cleanup reply count, and local cleanup complete. Debug logging
is needed for cleanup detail. Zero NTP replies is permitted and reported.
A timeout or local cleanup completion is never a remote acknowledgement.
The CLI reports `RECORD validated: true/false` even when subsequent cleanup fails.
Review errors as well as phase logs and exit status. Do not share raw payloads,
decrypted headers, credentials, keys, PINs, state files or unreviewed logs.

## One manually authorized Raspberry Pi attempt

**For the human operator only, after reviewing PR #35 and explicitly authorizing
this one attempt. Codex must not execute this procedure or contact a receiver.**
Target only the already approved Even-Olai HomePod `device:3ae5c9a302ad` (prior
operator endpoint `192.168.1.33:7000`, AudioAccessory5,1, firmware 27.2). Any
identity, endpoint, capability or policy change requires stopping and review.
Do not contact Stua Apple TV or any other receiver. Do not change production
OwnTone, autostream, systemd services or deployed binaries.

Use a separate checkout and verify its HEAD against the reviewed PR head:

```sh
git clone --branch feat/owntone-record-check --single-branch \
  https://github.com/kyrre-martila/PiAir.git PiAir-pr35
cd PiAir-pr35
git rev-parse HEAD
cargo build --release --locked
CONFIG=/absolute/path/to/existing/piair.toml
./target/release/piair --config "$CONFIG" config check
./target/release/piair --config "$CONFIG" receivers
```

Use the actual existing configuration and adjacent receiver-state directory;
never copy secrets, pair, unpair, replace credentials or create fresh test state.
Review existing approval and selection; only the HomePod may be contacted by
this diagnostic. `receivers` is passive discovery. Retain the correct existing
HomePod transient selection. If it is absent or changed, stop and obtain separate
operator authorization to review/pin selection; this procedure does not authorize
a change of profile or endpoint.

From an interactive terminal, execute **once**, with no playback options:

```sh
umask 077
./target/release/piair --config "$CONFIG" -v \
  stream-test device:3ae5c9a302ad --record-check --experimental-owntone-transient \
  2>piair-pr35-record.jsonl
result=$?
printf 'record-check exit=%s\n' "$result"
```

Read the full ID/profile/operation before answering `y`/`yes`; Enter/no/EOF
cancels before contact. Ctrl+C interrupts. Record commit, receiver metadata,
initial SETUP validation, event connection, NTP request/reply count, RECORD
status/validation, independent TEARDOWN outcome, local cleanup and exit status.
Stop after this attempt regardless of result: no retry, fallback, alternate
receiver, volume change or playback. Report sanitized summaries only.

## Limits and deterministic validation

Loopback tests use real transient SRP, independent receiver cipher state and
fragmented encrypted responses. They assert CSeq 1–7 on one control socket,
exact RECORD URI/body/headers, encrypted event ACK and closure, NTP replies
before SETUP response and during RECORD/TEARDOWN, rejection/timeout/EOF,
wrong/missing CSeq, HTTP response, duplicate/invalid length, transfer encoding,
bad tag, unsolicited frame bytes, body, latency and Session errors. They cover
TEARDOWN failure after RECORD success, event refusal/bad tag, cancellation and
approval revocation at SETUP/RECORD/TEARDOWN boundaries, byte-for-byte state
preservation and exact timing-socket release. Strict transcripts and the absent
encoder prevent SETPEERS, stream SETUP, volume, FLUSH, fallback and audio.
Existing PR #33/#34 and playback/worker tests remain regression gates.

Mocks establish implementation behavior only. Raspberry Pi ARM64 execution,
physical RECORD acceptance, actual event traffic, receiver-specific response
headers and remote resource expiry after failed cleanup remain uncertain.
Whether opening events is necessary for this HomePod, or whether RECORD affects
TEARDOWN behavior, remains a hypothesis. Even an acknowledged RECORD and
TEARDOWN establish neither stream negotiation nor audible playback.
