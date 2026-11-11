# Opt-in OwnTone transient initial SETUP check

This diagnostic is authentication plus **initial NTP SETUP only**. Successful
SETUP is not audible playback, stream readiness, persistent pairing, or durable
receiver identity verification. No physical success is claimed for this change.

Operator-confirmed PR #33 evidence on 2026-11-08: Even-Olai HomePod,
`device:<canonical-id>`, `192.168.1.33:7000`, AudioAccessory5,1 firmware 27.2:
transient SRP M1–M4, encrypted GET /info and initial NTP SETUP RTSP 200
succeeded, including plist/timing-profile/port validation. Encrypted TEARDOWN
CSeq 6 timed out after about three seconds with zero response bytes. No RECORD
or audio was sent. Transient SRP does not establish persistent receiver identity.
See the [pinned upstream comparison](OWNTONE_SETUP_TIMING_COMPARISON.md) for
observations and unresolved hypotheses.

## Scope and validation

The exact opt-in is:

```sh
piair stream-test 'device:<canonical-id>' --setup-check --experimental-owntone-transient
```

An explicitly chosen transient method is required. Supply the persisted canonical
ID or omit it to use the existing interactive picker. Unsupported capability/policy
states, persistent selection, playback mode and any `--setup-profile a|b|c`
combination fail without fallback. The picker never infers authentication from
names/models. See [receiver selection](RECEIVER_DIAGNOSTICS.md). Without the
flag, stream-test retains its legacy transient or persistent path. The separate
pyatv experiment remains authenticate-only. The existing optional
`--setup-timeout-seconds 15` remains explicitly scoped to initial SETUP; omit it
for this first attempt. No retry or profile cycling occurs.

Session owns the existing OwnTone preamble and transient proof flow, retaining
the same Wire/socket, profile, sender identity, authenticated directional keys,
frame counters and CSeq through:

1. Plaintext OwnTone-reference GET `/info` (CSeq 1), including strict policy checks.
2. Transient M1/M2 (CSeq 2), M3/M4 (CSeq 3); verify M4 before installing keys.
3. Encrypted GET `/info` (CSeq 4); validate existing audio/ALAC capabilities and NTP selection.
4. Encrypted initial NTP SETUP (CSeq 5), using the unchanged four-field offer, after the existing bounded NTP worker is ready.
5. Encrypted TEARDOWN (CSeq 6 on the complete sequence), then release local resources.

There is no OPTIONS in this setup-check sequence. A 200 alone is insufficient:
validate CSeq, RTSP framing, binary-plist Content-Type, bounded plist dictionary,
NTP timing profile (if supplied), required nonzero u16 `eventPort`, and any
optional nonzero u16 `timingPort`. This deliberately reuses existing NTP response
semantics; an omitted `timingProtocol` or `timingPort` is permitted. PTP fails.
No event connection, RECORD, stream SETUP, audio packets or volume changes occur.
No persistent enrollment, credential replacement or service changes occur.

## Receiver selection and operator usage

See [receiver discovery, selection and diagnostics](RECEIVER_DIAGNOSTICS.md)
for current commands, canonical ID placeholders, picker behavior and confirmations.
Use an independently verified allowlisted compatible receiver. No physical
receiver contact or Raspberry Pi service changes were performed for this change.

## Report phases and cleanup separately

Record commit, receiver ID/model/firmware/endpoint, profile, phase, numeric status,
proof/channel outcome, SETUP validation, TEARDOWN outcome and final exit status.
Stop on any rejection, timeout, closure, bad proof or malformed response. No
second attempt, alternate profile, playback or Apple TV contact is authorized by
these instructions.

- Transport/status rejection: numeric non-200 or peer closure is failure, even after authentication.
- Timeout: no response within the existing read/deadline bounds; it does not prove why the receiver stayed silent.
- Authenticated framing: `response_bytes` and `authenticated_frames` distinguish no bytes, incomplete/invalid frames and authenticated data. Invalid framing/tag/CSeq closes and invalidates the socket.
- Parsing/profile failure: authenticated RTSP 200 may still fail headers, plist, timing or ports. The existing cleanup attempts TEARDOWN on a usable connection.
- NTP: readiness is logged before initial SETUP. Request/reply events and the cleanup reply count distinguish traffic from no traffic; zero replies is permitted for setup-only.
- TEARDOWN: diagnostics separate request attempted from acknowledged, rejected, timed_out, connection_closed, other failed validation/I/O, or skipped because the control socket was invalidated. Local sockets/keys/workers are released in all cases. Skipped cleanup is not a receiver acknowledgement. Successful command output explicitly requires acknowledged TEARDOWN.

Share only reviewed phase/status/outcome summaries and sanitized application
errors. Do not share receiver-state files, credentials, PINs, keys, raw crypto
payloads, decrypted bodies/headers or packet captures. Debug logs include local
metadata and session URI; review before sharing. A failed cleanup leaves remote
resource release dependent on receiver connection/session expiry.

Loopback tests cover the exact same-socket sequence, independent continuous
cipher counters, CSeq, fragmented encrypted replies, policy/persistent rejection,
strict response validation, rejection, cancellation, allowlist revocation after
encrypted `/info`, credential preservation, timeout, framing and cleanup. Existing
legacy transient/persistent playback and lifecycle tests remain regression gates.
These tests establish implementation behavior only. Raspberry Pi ARM64 execution,
physical HomePod behavior with the earlier responder and audible playback remain unverified.
