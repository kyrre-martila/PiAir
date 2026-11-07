# Explicit pyatv transient HTTP framing and M1 ordering experiment

This is a controlled hardware experiment, not a compatibility fix. No hardware
success is established here; HomePod support is unverified. The normal transient
path remains unchanged and the experiment is disabled by default.

For an allowlisted receiver already explicitly selected with `--method transient`:

```sh
piair -v authenticate device:3ae5c9a302ad --experimental-pyatv-transient-preamble
```

An interactive terminal and exact receiver-ID confirmation are required before
discovery/connection. The flag belongs only to `authenticate`; non-transient
selection is rejected. It is never stored in configuration or selection. This
command creates no credentials and starts no audio. Optional `--control-check`
uses the existing encrypted OPTIONS diagnostic after verified M4 success.

## Pinned source investigation

Inspected pyatv revision `b277a4c8222ecdcbaab8a24e3e713ca44765adb4`:

- [`hap_transient.py`, `_AIRPLAY_HEADERS` and `verify_credentials`](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/auth/hap_transient.py)
  calls `http.post("/pair-pin-start", headers=_AIRPLAY_HEADERS)` with no body,
  before M1 and M3.
- [`http.py`, `post`, `send_and_receive`, `_format_message`](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/support/http.py)
  defaults to HTTP/1.1, converts supplied names to lowercase through `CaseInsensitiveDict`, and adds Content-Length
  only for a truthy body. No Host or CSeq is synthesized.

[`collections.py`, `CaseInsensitiveDict`](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/support/collections.py)
lowercases dictionary keys while preserving insertion order. PR #13 missed this
normalization; the complete experiment corrects casing for the preamble too.
Generated `Content-Length` retains its capitalization.

The experimental preamble is exactly:

```http
POST /pair-pin-start HTTP/1.1
user-agent: AirPlay/320.20
connection: keep-alive
x-apple-hkp: 4
content-type: application/octet-stream

```

The wire uses CRLF and an empty body. Host, CSeq and Content-Length are absent;
there is no PIN or TLV payload. This request does not consume PiAir's CSeq counter.
After a 200 response, M1 and M3 use the same pyatv profile, with
`Content-Length: <body byte count>` immediately after the request line and before
User-Agent. Host and CSeq remain absent. The length covers the encoded TLV bytes. PiAir retains its internal sequence accounting for subsequent control
requests, but sends no CSeq in experimental HTTP requests. SRP validation and
required receiver proof verification remain unchanged. The same
currently authorized numeric receiver connection is used throughout. Unlike the
pinned pyatv transient procedure, PiAir continues to verify M4's proof before
establishing directional encrypted control/event keys.

Any non-200 response stops immediately and closes the connection, including 403.
Existing bounded response parsing and security policy remain in force. There are
no retries, fallback, profile cycling, persistent credential writes, policy
changes or playback. OwnTone and Autostream are unchanged. The source provides
no firmware-specific explanation or undocumented requirements to implement.

## Diagnostics and validation

With `-v` or `-vv`, existing safe diagnostics report the `pyatv transient preamble`
response and the subsequent `pair-setup M1/M2` response. Without verbosity these
metadata logs remain disabled. Only allowlisted headers, status, safe reason,
phase and timing are emitted; response bodies, TLVs, proofs, keys, credentials,
cookies and arbitrary receiver text are never logged.

Loopback tests assert exact preamble bytes, HKP 4, empty body, request order,
exact experimental and default M1 body bytes, same-socket M1–M4 plus encrypted OPTIONS, immediate stop on 403/470/500 with no
fallback, non-transient rejection, safe logs and the normal transient regression.
These synthetic tests establish protocol behavior, not receiver compatibility.

## Scope and hardware findings

The existing `--experimental-pyatv-transient-preamble` name is retained for CLI
compatibility; it selects the complete preamble/M1/M3 HTTP profile and experimental M1 ordering. It is
explicit, disabled by default and never triggers detection or fallback.

Previous real HomePod mini testing (AudioAccessory5,1, firmware 27.2):

| Experiment | Response |
| --- | --- |
| Default PiAir transient M1 | HTTP 403 |
| pyatv HKP 4 `/pair-pin-start` | HTTP 200 |
| pyatv preamble + PiAir HTTP framing for M1 | HTTP 403 |
| pyatv preamble + pyatv HTTP framing for M1 (PR #14) | HTTP 400 |

PR #14 established a different response to the experimental HTTP profile, but
M1 was still rejected. These results do not identify the cause of rejection.
This M1 ordering experiment has no new hardware evidence and makes no HomePod
compatibility claim.

For both M1 and M3, the exact header sequence is:

```text
POST /pair-setup HTTP/1.1\r\n
Content-Length: <length>\r\n
user-agent: AirPlay/320.20\r\n
connection: keep-alive\r\n
x-apple-hkp: 4\r\n
content-type: application/octet-stream\r\n
\r\n
```

PR #15 changes only the experimental M1 TLV field order. In pinned pyatv,
`verify_credentials` constructs the dictionary in Method, SeqNo (State), Flags
order. [`hap_tlv8.py`, `write_tlv`](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/auth/hap_tlv8.py)
iterates `data.items()` in insertion order and writes each type, one-byte length,
and value. Method is 0x00, State is 0x06 and Flags is 0x13; the transient flag is
0x10. The experimental M1 is therefore exactly:

```text
00 01 00 06 01 01 13 01 10
```

Default PiAir M1 remains State/Method/Flags (`06 01 01 00 01 00 13 01 10`).
All field types, lengths and values are unchanged. The generic TLV encoder,
PR #14 HTTP framing, M3, SRP calculations, bounded parser, authorization checks,
receiver-proof verification, encryption and credential handling are unchanged.
In particular, the pinned pyatv transient routine returns after M3 without
verifying M4's receiver proof; PiAir still requires it.
Loopback validation checks exact M1 bytes in successful and rejected exchanges,
exact M1/M3 headers, request order, default regression, M3 rejection and invalid
receiver proof as well as preamble/M1 rejection. These are synthetic tests.
