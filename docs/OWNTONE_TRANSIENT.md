> Current guidance and evidence: [receiver authentication audit and safe Pi diagnostic](RECEIVER_AUTHENTICATION.md).
> Later operator evidence in [PR #21](INITIAL_SETUP_DIAGNOSTICS.md) reports HomePod
> transient SRP and encrypted OPTIONS success after #20. Earlier 400/403 results
> below are historical; no confirmed audible HomePod playback is documented.

# OwnTone-reference transient authentication experiment

HomePod transient authentication/control success is operator-reported for the tested receiver; playback remains unverified. This is an independent Rust protocol
implementation; no OwnTone code, architecture, identity or credentials are used.
The explicit option is `--experimental-owntone-transient`. Default CLI/config,
persistent HAP enrollment/pair-verify, and the labeled pyatv diagnostic remain
unchanged. This profile is available through `authenticate` and the explicitly opt-in
[initial SETUP-only diagnostic](OWNTONE_TRANSIENT_SETUP_CHECK.md). Normal playback
never uses this experimental profile.

## Local identity storage

`receiver-state/client-identity.toml` stores the full random 64-bit `client` as
exactly 16 uppercase hexadecimal digits in a TOML string; `remote` remains a
nonzero 32-bit integer. Existing nonzero numeric client identities are read
without rewriting or regenerating them. Invalid files fail closed and require
explicit investigation; they are never silently replaced. The existing owned
0700 directory, 0600 file checks and atomic no-clobber creation still apply.
Wire `Client-Instance` and `DACP-ID` retain uppercase hexadecimal formatting
without added leading zeroes, independently of the fixed-width storage format.

Initialization audit: saved selection `capabilities.features` also spans all
64 bits and uses the same lossless string format with legacy numeric reads.
The remaining persisted unsigned values fit TOML integers: `remote` and status
flags are at most 32 bits, configuration durations are validated with small
bounds, and nonzero `acl`/`act` values are rejected before selection is saved.
Credentials contain strings, a version byte and byte arrays. Session counters,
random wire identifiers and binary-plist integers are not serialized to TOML.

## Verified reference

All references below are to OwnTone commit
[`f7ace58dc2aeb5305360770da37fd62095f5bb21`](https://github.com/owntone/owntone-server/tree/f7ace58dc2aeb5305360770da37fd62095f5bb21).
Line numbers refer to that revision, not a moving branch.

| Protocol fact | Source evidence | PiAir behavior |
| --- | --- | --- |
| Read policy before pairing | `src/outputs/airplay.c:3273–3359`, `response_handler_info_generic`; sequence table at 3634 onward | GET `/info` on the authorized socket; require bounded binary plist dictionary and unsigned `statusFlags` |
| Policy precedence: one-time pairing bit 9, PIN bit 3, password bit 7, otherwise transient | `airplay.c:181–195`, 3310–3359 | Compare these bits with pinned mDNS policy and reject all three requirements; never select another method |
| No PIN-start on the unprotected transient path | `airplay.c:3356–3359`, sequence table 3671, 3697–3698 | Direct `/info` → transient M1; no pyatv preamble |
| Header insertion and CSeq starting at 1 | `airplay.c:886–935`, 1602, 3787–3824 | Same ordering and reference AirPlay User-Agent; PiAir name and independent persisted random public sender IDs |
| HKP 4 on transient setup, content type octet-stream | `airplay.c:2821–2892`, 3697–3698 | Both M1 and M3 |
| RTSP version, body length, header serialization | `src/evrtsp/rtsp.c:327–371` | RTSP/1.0; length appended after application headers for nonempty body; GET has no body, length, Host or HKP header |
| State/Method/Flags insertion order | `src/pair_ap/pair_homekit.c:1230–1274`; `src/pair_ap/pair-tlv.c:37–58`, 116–160 | M1 bytes below; existing Rust TLV encoder preserves order and fragments values at 255 bytes |
| SRP-3072/SHA-512, user Pair-Setup, transient PIN 3939 | `pair_homekit.c:1184–1185`, 1250, 1290–1331 | Reuse existing Rust arithmetic, independent checked-in SRP/proof/key fixtures |
| M2 salt 16 bytes, B at most 384; M3 State/A/proof | `pair_homekit.c:1440–1484`, 439–467, 1285–1331 | Validate parameters and existing proof/state/TLV constraints |
| Require M4 64-byte receiver proof and verify it before completing | `pair_homekit.c:1488–1534` | Existing constant-time proof verification before any channel key activation |
| Initialize cipher immediately after verified M4; use full SRP session key for control HKDF | `airplay.c:3444–3487`, 1438–1475; `pair_homekit.c:2953–3010` | Reuse existing directional Control-Salt write/read HKDF and framed ChaCha20-Poly1305; retain the same TCP socket |
| OwnTone may switch to PIN on 470 | `airplay.c:3424–3439`, 3696 | Deliberate deviation: fail closed, close socket, no retry or fallback |
| OwnTone tolerates response CSeq mismatch, absent length means zero | `airplay.c:3735–3737`; `rtsp.c:1146–1149` | Deliberately stricter: require matching RTSP CSeq and explicit length in plaintext authentication responses |

Source files:
[airplay.c](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c),
[rtsp.c](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/evrtsp/rtsp.c),
[pair_homekit.c](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/pair_ap/pair_homekit.c),
[pair-tlv.c](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/pair_ap/pair-tlv.c).

OwnTone queues requests on the same `session->ctrl` connection, waits for each
response, then advances the sequence (`sequence_continue`, `sequence_continue_cb`).
PiAir instead retains its consuming Rust Wire/Session ownership and bounded parser;
errors drop the socket and secret owners. No reference implementation logging of
raw pairing responses is reproduced.

## Exact wire contract

Lines below end in CRLF, including the final empty line. `<client>` is PiAir's
uppercase unpadded hex u64, shared by Client-Instance/DACP-ID. `<remote>` is its
own decimal u32. They are generated once and stored in the existing protected
`receiver-state/client-identity.toml` (0600, private 0700 directory). They are
public identifiers, independent of persistent pairing credentials. They remain
stable across experiment invocations; SRP private material remains fresh.

```text
GET /info RTSP/1.0\r\n
CSeq: 1\r\n
User-Agent: AirPlay/999.0.0\r\n
X-Apple-Client-Name: PiAir\r\n
Client-Instance: <client>\r\n
DACP-ID: <client>\r\n
Active-Remote: <remote>\r\n
\r\n
```

For M1 and M3, respectively `<seq>` is 2 and 3:

```text
POST /pair-setup RTSP/1.0\r\n
CSeq: <seq>\r\n
User-Agent: AirPlay/999.0.0\r\n
X-Apple-Client-Name: PiAir\r\n
Client-Instance: <client>\r\n
DACP-ID: <client>\r\n
Active-Remote: <remote>\r\n
Content-Type: application/octet-stream\r\n
X-Apple-HKP: 4\r\n
Content-Length: <encoded body length>\r\n
\r\n
```

M1 length is 9, body exactly `06 01 01 00 01 00 13 01 10`.
M2 must have State=2, salt and B; optional Flags must equal 0x10.
M3 is State=3, PublicKey=A (type 03, minimal unsigned integer, chunked at 255), Proof (type 04,
length 64). M4 must have State=4 and the valid 64-byte receiver Proof. No
M5/M6, pair-verify, credentials, or playback follow. Optional encrypted OPTIONS
is a PiAir diagnostic on this same socket, CSeq=4; it is not required for SRP
success and does not prove audio compatibility.

`/info statusFlags` is not mDNS `flags`/`sf`. PiAir compares only established
authentication bits 3/7/9, not the full words: operational bits can differ.
Missing, wrongly typed, negative, over-32-bit flags, malformed/big/cyclic plist,
contradictory required methods, and nonzero explicit access policy are rejected.
The saved mDNS access/password/feature policy is also revalidated. As elsewhere
in PiAir, unknown non-pairing flags are retained without invented semantics;
bit 10 alone does not establish mandatory Home membership. Undocumented access
requirements cannot be inferred from a single flag or successful `/info`.

Header/body limits are 8192 bytes; the existing binary-plist preflight limits
object counts, collection sizes, depth and expansion before decoding. Responses
may fragment anywhere. Duplicate headers, mismatched/absent RTSP CSeq, redirects,
transfer encodings, closed connections, non-200 status and invalid TLV stop the
exchange. Receiver-controlled body/proof/keys/reason text/cookies are not logged.

## Hardware evidence and explicitly authorized command

Supplied HomePod mini evidence: AudioAccessory5,1, firmware 27.2. PR discussions
#12–15 contain no additional recorded comments; the following is the supplied
hardware evidence and repository experiment history, not newly reproduced here.

| Stage | Observed outcome |
| --- | --- |
| PR #12 diagnostics / earlier default transient attempts | M1 rejected with 403 |
| PR #13 pyatv PIN-start + PiAir M1 framing | PIN-start 200, M1 403 |
| PR #14 complete pyatv HTTP framing | PIN-start 200, M1 400 |
| PR #15 pyatv M1 Method/State/Flags ordering | Current supplied evidence still reports M1 400 after PIN-start 200; no verified transient authentication |
| OwnTone RTSP profile after PR #17 | Supplied first hardware test reaches HomePod; initial GET /info returns RTSP/1.0 403 Forbidden; no pairing exchange reached |

The prompt does not provide an isolated PR #15 trace, so attribution of the
latest 400 to ordering specifically remains uncertain. No accepted preamble or
single message establishes authentication. No firmware-specific explanation for
400/403 is established by these sources.

Only after separately authorizing the exact HomePod, allowlisting it, and
explicitly selecting it with `--method transient`, run in an interactive terminal:

```sh
piair -v authenticate device:3ae5c9a302ad --experimental-owntone-transient
```

Confirm the exact receiver ID at the prompt. This invocation discovers and
checks the pinned identity, capabilities, endpoint and current approval before
connecting. If your approved receiver ID differs, use that exact approved ID.
For an explicitly desired encrypted-control diagnostic, append `--control-check`.
Do not run playback/stream-test as part of this experiment.

Success requires policy acceptance, M2 validation, M4 receiver-proof verification,
and local channel initialization; `--control-check` additionally requires a
valid encrypted matching OPTIONS response. This authenticates a connection,
not a persistent HomePod identity. Failure at any stage, including 400/403/470,
malformed messages, timeout or invalid proof, ends the attempt without fallback.
Report phase/status and model/firmware only; never publish secrets or raw pairing
payloads. Different authentication requires separate explicit user approval.
OwnTone/Autostream services, settings, databases and credentials are untouched.

See [the PR #18 investigation](HOMEPOD_INFO_403.md) for the verified User-Agent mismatch, complete discovery/connection trace, limitations and retest procedure.
