# Explicit pyatv transient preamble experiment

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
  defaults to HTTP/1.1, preserves the supplied headers, and adds Content-Length
  only for a truthy body. No Host or CSeq is synthesized.

The experimental preamble is exactly:

```http
POST /pair-pin-start HTTP/1.1
User-Agent: AirPlay/320.20
Connection: keep-alive
X-Apple-HKP: 4
Content-Type: application/octet-stream

```

The wire uses CRLF and an empty body. Host, CSeq and Content-Length are absent;
there is no PIN or TLV payload. This request does not consume PiAir's CSeq counter.
After a 200 response, the existing PiAir M1–M4 requests retain their original
headers, SRP validation and required receiver proof verification. The same
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
same-socket M1–M4 plus encrypted OPTIONS, immediate stop on 403/470/500 with no
fallback, non-transient rejection, safe logs and the normal transient regression.
These synthetic tests establish protocol behavior, not receiver compatibility.
