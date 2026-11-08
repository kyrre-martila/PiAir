# OwnTone setup-only timing comparison

Inspected before implementation against fresh PiAir `origin/main`
`b571194b55edf80037825da6f963267d8fa82ba8` (merged PR #33).
OwnTone upstream HEAD was pinned on 2026-11-08 to
[`f7ace58dc2aeb5305360770da37fd62095f5bb21`](https://github.com/owntone/owntone-server/tree/f7ace58dc2aeb5305360770da37fd62095f5bb21).
The references below are to that exact revision of `src/outputs/airplay.c`.
These are source observations, not receiver acceptance evidence.

| Area | Verified OwnTone behavior | Verified PiAir behavior / focused change |
| --- | --- | --- |
| Service lifetime | [`airplay_init`, L4318](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L4318) starts shared timing service before playback sessions; deinit/error paths stop it (L4362, L4376). | `Session::negotiate` binds per-session sockets after authenticated GET /info. PR #33 setup-only profile A deferred the responder; explicit OwnTone setup-only now starts the existing worker before initial SETUP. Legacy A remains deferred; legacy B/C and playback remain early. |
| NTP packets | [`timing_svc_cb`, L2262](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L2262) checks 32 bytes and `80 d2`, sends `80 d3` to recvfrom source; echoes byte 2 and originate timestamp. | `realtime::timing_reply` retains its existing full two-byte sequence echo and timestamps. `Wire::start_timing` moves the bound socket once into the existing bounded worker. Only the pinned approved control peer IP can receive replies at its query source port; lease is checked before sending. UDP itself is unauthenticated. OwnTone callback has no corresponding peer-IP/approval filter. |
| Initial offer | [`payload_make_setup_session_ntp`, L2691](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L2691) sends deviceID, sessionUUID, timingPort, timingProtocol=NTP. | `profile::ntp_offer` already sends the same four fields with their established types. No serializer, authentication, cipher, framing, keys or offer changes. |
| Initial response | [`response_handler_setup_session`, L3180](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L3180) requires nonzero eventPort; connects events; timingPeerInfo is PTP-only. | `Session` requires bounded binary plist, NTP if specified, valid u16 eventPort and optional timingPort. Setup-only makes no event connection. Worker readiness requires no incoming NTP query. |
| Request sequence | [Sequence table, L3640](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L3640) normally continues SETUP(session), RECORD, SETPEERS, SETUP(stream), volume. | OwnTone setup-only ends immediately after validated initial SETUP with encrypted TEARDOWN CSeq 6 on the same Wire/socket, continuing directional cipher counters. No RECORD, SETPEERS, stream SETUP, volume or audio. |
| TEARDOWN body/lifecycle | [`payload_make_teardown`, L2423](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L2423) serializes an empty binary dictionary and immediately marks TEARDOWN. [Handlers, L3259](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L3259) mark STOPPED for normal stop or FAILED (except AUTH) for failure. | Existing empty binary dictionary retained. Pending Ports retains the responder through TEARDOWN, then local cleanup drops/joins it. Failure cleanup limits the remaining budget and skips invalidated control. Revoked approval prohibits further contact. |
| Response policy | [`sequence_continue_cb`, L3708](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L3708) fails on missing response. Normal stop permits non-200 to proceed; failure TEARDOWN does not (sequence table). Explicitly does not validate reply CSeq. | PiAir requires RTSP 200, matching CSeq and existing framing/body validation. Non-200 is rejected; timeout/EOF remain failures. Local cleanup does not imply receiver acknowledgement. No retry, timeout increase or fallback. |
| Session headers | [`request_headers_add`, L889](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L889) adds CSeq, sender/client headers and optional auth/stream ID, no RTSP Session header. `sequence_continue` uses session URL (L3810). | PiAir `Session::call` validates and retains a returned Session token and propagates it into SETUP/TEARDOWN with the same RTSP URI. Tests assert this continuity. This existing difference is preserved. |

Reviewed local `session.rs`, `wire.rs`, `profile.rs`, `realtime.rs`,
`stream_tests.rs` and the setup, timing, response, protocol-audit and OwnTone docs.
Earlier docs describe historical legacy A/B/C behavior; this explicit OwnTone
opt-in is the only lifecycle exception introduced here.

## Physical observations versus hypotheses

Operator-reported 2026-11-08 Even-Olai HomePod result for PR #33: transient SRP
M1–M4 succeeded, encrypted GET /info succeeded, initial NTP SETUP returned RTSP
200 with validated binary plist, timing profile and ports. Encrypted TEARDOWN
CSeq 6 timed out after about three seconds with zero response bytes. No RECORD
or audio was sent. This is physical initial-SETUP evidence for the prior build.

Whether earlier NTP readiness changes this receiver's TEARDOWN behavior is an
untested hypothesis. Neither the timeout nor the source comparison proves NTP
was its cause, that any NTP query was sent, or that an incomplete session must
acknowledge TEARDOWN. OwnTone's ordinary playback-stop path is a different
session progression. No physical success is claimed for this change; Raspberry
Pi execution and receiver response require the single operator test in
[the procedure](OWNTONE_TRANSIENT_SETUP_CHECK.md).

## Diagnostic interpretation

Structured records separate initial SETUP response/profile validation, NTP ready,
NTP request/reply events, TEARDOWN `attempted`, then `acknowledged`, `rejected`,
`timed_out`, `connection_closed`, `failed` (other validation/I/O), or `skipped`
(invalidated transport), and local cleanup completion. The setup-only NTP cleanup
summary reports whether a worker started and its reply count, including zero.
Request/reply records carry no raw packet contents. Existing sanitized control
records retain byte/frame counts and numeric status. A timeout never becomes an
acknowledgement or a successful command exit. Local cleanup completes even when
remote cleanup fails; remote resource expiry remains receiver-dependent.
