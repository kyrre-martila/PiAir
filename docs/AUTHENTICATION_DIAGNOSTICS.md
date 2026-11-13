> Current follow-up: [opt-in HomePod transient initial SETUP-only check](OWNTONE_TRANSIENT_SETUP_CHECK.md).

Current receiver discovery, selection and command usage is documented in
[Receiver diagnostics](RECEIVER_DIAGNOSTICS.md). Receiver-specific observations
and authorizations below are historical evidence, not current CLI restrictions.
Examples use placeholders; verify the canonical ID independently.
> Authentication/control success on 2026-11-08 does not establish SETUP or playback.

> Current guidance and evidence: [receiver authentication audit and safe Pi diagnostic](RECEIVER_AUTHENTICATION.md).
> Later operator evidence in [PR #21](INITIAL_SETUP_DIAGNOSTICS.md) reports HomePod
> transient SRP and encrypted OPTIONS success after #20. Earlier 400/403 results
> below are historical; no confirmed audible HomePod playback is documented.

# AirPlay authentication diagnostics

This change does not fix or establish HomePod compatibility. No receiver was
contacted during development. The early hardware observation was:
Example HomePod, `device:<canonical-id>`, `192.0.2.33:7000`,
`AudioAccessory5,1`, firmware 27.2, AirPlay protocol 1.1, status `0x98404`,
ACL 0. Persistent HKP 3 PIN-start returns HTTP 403; explicitly selected transient
HKP 4 passes local capability policy but its first `/pair-setup` returns HTTP 403.
A rejection before M2 cannot be attributed to SRP proof calculation or session
key derivation, because neither has been reached. It does not reveal the server's
policy reason. ACL 0 and capability checks are not permission guarantees.

## Historical legacy-profile reproduction (not the recommended next test)

Use the existing configuration, approval and selected method. Do not unpair,
change credentials or receiver access policy. Inspect passive discovery first:

```sh
piair devices
piair receivers
# For the already selected transient method:
piair -v authenticate 'device:<canonical-id>' 2>auth-diagnostics.jsonl
```

`-v` enables DEBUG JSON on stderr; `-vv` also enables TRACE and adds no raw
payload logging. `--config /path/to/config.toml` may be supplied as usual.
Confirm the exact receiver ID interactively when requested. Do not use
`--control-check` or `stream-test` for this reproduction. Stop on rejection.
There is no fallback, retry, variant negotiation, playback or credential write.
The existing `pair` command with an already selected `pin-display` method can
record PIN-start failure with `-v`; do not start enrollment merely to collect
logs. It could save credentials if enrollment unexpectedly succeeds.

Record the selected method, model/OS/firmware advertisement fields, phase,
status and safe headers. Missing advertisement fields remain missing; `srcvers`
is the AirPlay software version and must not be relabeled as OS firmware.
Review logs before sharing: they still contain local endpoint and device metadata.
Do not share packet captures, PINs, credentials or raw upstream debug logs.

## Diagnostic boundary

`Wire::post_inner` logs the actual constructed HTTP request line, numeric Host,
body length and allowed headers, after endpoint validation. `pairing::exchange`
sets an explicit M1/M2, M3/M4, M5/M6 or pair-verify phase without parsing payloads
for logging. PIN-start has a separate phase. Each exchange records duration,
total connection duration and success; transport events distinguish deadline,
read/write error kind, peer EOF and local close. Selected method and advertised
policy are logged before connection. The normal INFO level emits none of these
new diagnostics. Connection creation failures report elapsed time and error kind.

Header name **and value** allowlists apply case-insensitively to names:

| Header | Permitted values |
|---|---|
| CSeq, Content-Length, X-Apple-HKP | 1–10 decimal digits |
| Connection | close or keep-alive |
| Content-Type | application/octet-stream or application/x-apple-binary-plist |
| User-Agent | exact PiAir/0.1, AirPlay/320.20 or AirPlay/999.0.0 |

Numeric Host is emitted separately from the local socket endpoint, never a
receiver-provided Host value. Unknown names/values are omitted entirely, including
Server, Location, WWW-Authenticate, Authorization, cookies and tokens. Standard
200 OK, 403 Forbidden and 470 Connection Authorization Required reasons are
allowlisted; other reason text is replaced with `unrecognized (omitted)`.
Untrusted reasons may reflect credentials. No body contents, TLVs, SRP fields,
proofs, keys, PINs or credentials are passed to tracing. Non-200 response metadata
is logged before the existing immediate status rejection; its body is not read.
Existing response framing/acceptance rules remain unchanged.

## Immutable implementation comparison

Inspected sources at fixed revisions:

- OwnTone [airplay.c](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c),
  `request_headers_add`, `payload_make_pin_start`, `payload_make_pair_generic`,
  `AIRPLAY_SEQ_PAIR_TRANSIENT`, `sequence_continue`;
  [rtsp.c](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/evrtsp/rtsp.c), `evrtsp_make_header_request`.
- pyatv [hap_transient.py](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/auth/hap_transient.py),
  [hap.py](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/auth/hap.py),
  [http.py](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/support/http.py),
  [utils.py](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/utils.py).
- PiAir baseline [wire.rs](https://github.com/kyrre-martila/PiAir/blob/cde3f77ac93fff550c6e836e86462aaa22e4e08d/src/receiver/wire.rs)
  and [pairing.rs](https://github.com/kyrre-martila/PiAir/blob/cde3f77ac93fff550c6e836e86462aaa22e4e08d/src/receiver/pairing.rs).

The PiAir column below describes the unchanged default profile. The explicit
[HTTP framing and M1 ordering experiment](PYATV_TRANSIENT_PREAMBLE.md) uses pyatv
headers for preamble, M1 and M3, plus Method/State/Flags ordering for M1 only.
Its diagnostic record includes the prior HomePod mini 403/200/403/400 results;
there is no new hardware success evidence.

| Detail | OwnTone | pyatv transient | PiAir |
|---|---|---|---|
| Transient sequence | two POST /pair-setup | POST /pair-pin-start then two POST /pair-setup | two POST /pair-setup |
| Transient HKP | 4 | 4, including preamble | 4 |
| Authentication request protocol | evrtsp emits RTSP | HTTP/1.1 default | HTTP/1.1 |
| User-Agent | configured airplay_shared user_agent; pinned default AirPlay/999.0.0 | AirPlay/320.20 | PiAir/0.1 (legacy); AirPlay/999.0.0 (OwnTone experiment) |
| Client identity headers during authentication | CSeq, X-Apple-Client-Name, Client-Instance, DACP-ID, Active-Remote | none in transient header map | CSeq only |
| Connection/Host | evrtsp transport | keep-alive; no Host in formatter/header map | keep-alive, numeric Host |
| Content-Type | octet-stream for setup; PIN-start sequence has no type | octet-stream, including empty preamble | octet-stream, including empty PIN-start |
| Content-Length | evrtsp adds for nonempty body | formatter adds only for truthy body | explicit including zero |
| Transient M4 proof | validated | POST result awaited, proof not inspected here | validated |

All use transient method 0, state 1, flag 0x10, then SRP public/proof state 3;
PiAir preserves the existing implementation and validation. Persistent PiAir
pin-display uses HKP 3 PIN-start then M1–M6; setup-code skips PIN-start;
existing credentials use pair-verify M1–M4. OwnTone has a distinct persistent
PIN-start sequence and HKP 3 normal pairing. pyatv's persistent `hap.py` uses HKP 3,
`AirPlay/320.20`, PIN-start and persistent setup exchanges. These sequences are
not interchangeable. See [TRANSIENT_AUTHENTICATION.md](TRANSIENT_AUTHENTICATION.md)
for cryptographic comparison and OwnTone's 470 routing, which PiAir does not adopt.

pyatv's preamble is a concrete difference worth a separately authorized experiment.
The pinned implementation does not condition it on firmware 27.2 or explain a
27.2-specific requirement. Neither source inspection nor a 403 establishes that
adding it would work. PiAir's HKP 3 PIN-start rejection is not a test of pyatv's
HKP 4 preamble. The separately opt-in [controlled HTTP framing experiment](PYATV_TRANSIENT_PREAMBLE.md)
now implements this exact HKP 4 preamble; the default path remains unchanged. Header,
protocol and Home access-policy differences remain independent hypotheses.

## Discovery investigation

PiAir already retains `features/ft`, `flags/sf`, `pw`, `acl`, `act`, `protovers`,
`pi`, `model/am`, `osvers` and `srcvers`. Security fields are parsed and pinned;
act was added in PR #11. pyatv checks password bit 0x80, PIN bit 0x8, persistent
pairing bit 0x200; ACL 1 disables pairing, and act 2 is treated as unsupported
(possibly Current User, explicitly uncertain upstream). PiAir conservatively
rejects all nonzero ACL/act. The supplied `0x98404` has none of bits 0x8, 0x80,
0x200; it still does not prove transient authorization.

OwnTone's discovery examples include `fv` firmware strings that PiAir previously
ignored. This change retains bounded printable `fv` as informational metadata
and logs it alongside osvers/srcvers; it does not affect selection or policy.
Other advertisements such as pk, group identifiers, serial numbers and fex are
not collected or dumped. Their presence does not establish Home authorization.
OwnTone can obtain statusFlags through a later /info response; adding a probe
would change the network sequence, so none is added. No ignored field identified
here provides a documented explanation for this HomePod's first-request 403.

Tests use synthetic loopback receivers, including 403 with secret/unexpected
headers, timeout, EOF, safe diagnostic capture, INFO suppression and the existing
persistent/transient authentication regression suite. No OwnTone or Autostream
files or services are changed.

The table above describes the original legacy framing comparison. The explicit
OwnTone experiment and its initial GET /info are documented separately in
[OWNTONE_TRANSIENT.md](OWNTONE_TRANSIENT.md). PR #18 identified the pinned
OwnTone User-Agent default as a supported explanation for the new GET /info 403;
see [the full investigation and retest instructions](HOMEPOD_INFO_403.md). Debug
requests now mark `headers_filtered=true`; omitted headers may still be on the
wire. After authorization checks, an endpoint event records the selected AirPlay
service and numeric peer before the single TCP connection attempt.
