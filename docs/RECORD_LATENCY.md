# RECORD latency evidence and diagnostics

Based on fresh origin/main `b82c5be` (merged #26). Reported Apple TV Stua
(AppleTV6,2, tvOS 26.6.1) results: authentication, encrypted initial SETUP and
early NTP succeed; encrypted RECORD returns 200 in about 15 ms, then latency
validation rejects it. Encrypted TEARDOWN succeeds; no audio packets are sent.
The actual Audio-Latency value is not yet known. No hardware playback was
attempted for this change; no production services were modified or restarted.

## Source comparison

Inspected OwnTone **29.3**, pinned to
[`d6fb3edf5831de38134ebd92fcf09a730ddd37aa`](https://github.com/owntone/owntone-server/tree/d6fb3edf5831de38134ebd92fcf09a730ddd37aa):

- [`airplay.c:3031`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L3031):
  `response_handler_record` only sets RECORD state and continues. It does not
  parse or validate Audio-Latency. Searching this file finds Audio-Latency only
  in the explanatory comment at line 91, not in header handling.
- [`airplay.c:2628`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L2628):
  stream SETUP offers latencyMax=88200 and latencyMin=11025, with a TODO asking
  how these latencies work. An offered stream range is not evidence that an
  earlier RECORD receiver report must lie within that range.
- [`airplay.c:91`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L91):
  describes 11025 as a guess based on an AirPort Express report.
- [`airplay.c:1196`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L1196):
  sender output buffering subtracts a fixed 250 ms from configured buffering.
  This does not establish support for arbitrary receiver-reported latency.

PiAir's `profile.rs` emits and validates echoed stream latency fields separately.
`session.rs` calls `realtime::latency` immediately after RECORD, before stream
SETUP. Missing headers default to 11025; present headers require unsigned ASCII
decimal fitting u32 and the inclusive 11025..88200 range. Later transmission
subtracts this value from 88200 for sync and drains two seconds of silence.
`wire.rs` normalizes names, trims surrounding whitespace and rejects duplicate
headers before latency validation. Existing encrypted stream tests exercise
RECORD=11025, the stream offer, timing, media and cleanup.

## Decision

These numbers are sender choices, not demonstrated protocol limits. Applying
them to RECORD is an unproven assumption, potentially unnecessarily restrictive.
However, OwnTone's decision not to consume this header does not justify changing
PiAir's timing model or ignoring a malformed report. Values above 88200 would
also underflow the current sync subtraction. Preserve validation, default,
errors and all session behavior until the actual report and timing semantics
are established. No new values are supported, no clamping or fallback on invalid
headers is introduced. No OwnTone implementation code was copied.

At debug verbosity (`-v`), one fixed RECORD diagnostic reports header presence,
parsed u32 value (including rejected numeric values), inclusive validation bounds,
missing-header fallback and a fixed rejection reason. Malformed/overflow values
are omitted. Raw header text, other headers, encrypted payloads, credentials,
keys and session identifiers are never passed to this diagnostic. Values outside
u32 are classified as overflow, not logged as arbitrary strings.

Deterministic tests cover absent, empty, malformed, signed, fractional, embedded
whitespace, overflow, zero, both boundaries, adjacent out-of-range values, an
interior value and u32::MAX. They also verify debug-only emission and omission
of supplied secrets. Hardware latency and physical playback remain unverified.
