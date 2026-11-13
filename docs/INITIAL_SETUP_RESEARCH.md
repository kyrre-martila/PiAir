# Initial realtime audio SETUP investigation (PR #10)

Current receiver discovery, selection and command usage is documented in
[Receiver diagnostics](RECEIVER_DIAGNOSTICS.md). Receiver-specific observations
and authorizations below are historical evidence, not current CLI restrictions.
Examples use placeholders; verify the canonical ID independently.

Historical findings below describe PR #10. See [PR #21 investigation](INITIAL_SETUP_DIAGNOSTICS.md)
for persistent sender identity, current diagnostics and the initial-SETUP-only hardware check.

## Evidence and diagnosis

Baseline: `ab5d000`, fresh main after merged PR #9. Hardware observations supplied
by the operator: AppleTV6,2 / tvOS 26.6.1, HAP pairing and pair-verify, encrypted
OPTIONS and GET /info succeed; initial SETUP times out reading response headers;
zero audio packets sent. No hardware was contacted for this investigation.

The strongest concrete request discrepancy is missing sender identity headers.
Both inspected audio senders send Client-Instance, DACP-ID and Active-Remote.
OwnTone additionally sends X-Apple-Client-Name. PiAir sent none. This is the leading
request-construction hypothesis, **not a proven explanation of the timeout**.
A timeout alone does not identify a missing header or prove PTP is required.
The correction restores this common sender behavior without inventing plist fields.
Hardware validation is still required before calling the negotiation failure fixed.

The initial NTP plist is already the exact four-field shape used by OwnTone.
The IPv4 URI and HAP framing also match inspected implementations. No evidence
supports changing the IPv4 URI to the receiver address, `/stream`, or `*`, adding
remote-control fields, increasing timeouts, or switching protocol versions.
OwnTone brackets IPv6 literals; PiAir currently does not. That separate IPv6 URI
limitation cannot explain an IPv4 attempt and is outside this correction.

## Source comparison (verified code behavior)

Sources were inspected at the immutable revisions linked below; these are source
observations, not claims of interoperability testing against this tvOS release.

| Aspect | OwnTone audio sender | pyatv audio sender | PiAir and correction |
| --- | --- | --- | --- |
| URI | Local sender IP and random unsigned 32-bit session ID; IPv6 bracketed | Local sender IP and random 32-bit ID | Same IPv4 shape, unchanged |
| Headers | CSeq, User-Agent, X-Apple-Client-Name, Client-Instance, DACP-ID, Active-Remote; Content-Type for plist, automatic Content-Length | Common RTSP layer sends CSeq, DACP-ID, Active-Remote, Client-Instance; dict bodies become binary plist with Content-Type | Add stable per-control-connection random 64-bit hex Client-Instance/DACP-ID, random decimal u32 Active-Remote and truthful name PiAir |
| NTP base plist | deviceID string, sessionUUID string, timingProtocol string NTP, timingPort unsigned integer | Same four plus sender metadata, multi-select/group booleans and iPhone identity strings | Preserve OwnTone's minimal four fields and integer port; no iPhone impersonation |
| Identity | Sender device ID, separate session UUID, random URI ID; library hash used for matching Client-Instance/DACP-ID | Fixed synthetic MAC, fresh UUID, random RTSP ID and matching random client/DACP ID | Existing locally administered random device ID, fresh UUID, random URI ID retained; header IDs stable during connection |
| Authentication | Pair-verify obtains shared secret then sets up encrypted control | Audio _setup_base verifies connection before SETUP | Existing pinned persistent HAP verify and encrypted control retained |
| Timing | Explicit NTP branch; PTP preferred if advertised unless disabled | AirPlayV2 audio offers NTP | NTP only; no automatic PTP or legacy fallback |
| Ordering | Session SETUP, RECORD, PTP-only SETPEERS, stream SETUP, volume | Base SETUP and stream SETUP in setup(); RECORD later in stream_client | Preserve OwnTone NTP order; later ordering cannot explain timeout on first SETUP |
| Responses | Session SETUP must return 200 and parseable plist with nonzero eventPort; opens encrypted events. Stream SETUP validates stream ports | Base reads eventPort and opens encrypted events; stream reads streams[0].controlPort/dataPort | Strict 200/CSeq, binary plist, bounded ports, NTP timingPort and eventPort retained; explicit non-NTP response rejected |
| Version | No X-Apple-ProtocolVersion added by request_headers_add | Common RTSP exchange does not add it; airplayv2.py defines HEADERS with version 1, session/stream headers but _setup_base does not pass that constant | Existing version 1 retained; no evidence that version 2 is needed |

### OwnTone references

- [src/outputs/airplay.c:889 request_headers_add](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L889): actual sender headers and types.
- [session identity and URL:1530](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L1530).
- [NTP session offer:2690](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L2690) and [separate PTP offer:2721](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L2721).
- [session response:3180](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L3180), [pair-verify upgrade:3548](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L3548), [startup sequence:3640](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L3640), [request URI selection:3815](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L3815).
- [PTP preference:4070](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L4070). SupportsPTP is a capability, not evidence that NTP is forbidden.
- [control encryption hook:1000](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L1000) and [src/pair_ap/pair_homekit.c:3019](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/pair_ap/pair_homekit.c#L3019): little-endian two-byte block length, max 1024 plaintext bytes, ChaCha20-Poly1305, length AAD, directional keys, incrementing nonce counter. Matches PiAir; independent fixtures verify bytes.

### pyatv references and exclusions

- [pyatv/protocols/raop/protocols/airplayv2.py:51](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/raop/protocols/airplayv2.py#L51): actual AUDIO base SETUP; [stream SETUP:112](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/raop/protocols/airplayv2.py#L112) uses realtime type 96 but PCM (`ct=1`, format 0x800), not PiAir ALAC.
- [pyatv/support/rtsp.py:80](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/support/rtsp.py#L80) and [exchange:264](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/support/rtsp.py#L264): URI, client IDs, headers and binary serialization.
- [pyatv/protocols/raop/stream_client.py:438](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/raop/stream_client.py#L438): RECORD ordering.
- [pyatv/auth/hap_session.py](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/auth/hap_session.py): independent control framing implementation.
- [pyatv/protocols/airplay/ap2_session.py:75](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/ap2_session.py#L75) is explicitly **remote control**: timingProtocol None and remote-control setup are not an audio reference. Its fields were not copied.

Receiver projects such as RPiPlay and shairport-sync do not establish working
sender request construction and were not used as sender evidence. OwnTone's
NTP/ALAC branch and pyatv's NTP/realtime audio branch are the relevant independent
sender references. Their successful use elsewhere does not certify this Apple TV.

## Changes, diagnostics and limits

Only authenticated session RTSP requests gain the four sender headers; pair-setup,
pair-verify and OPTIONS behavior are unchanged. The IDs are fresh for each Wire,
remain stable across GET/SETUP/RECORD/cleanup, and carry no credentials. They do
not advertise a DACP server or add inbound access. PiAir retains its own User-Agent.

Request validation/build failures now carry `request construction`; encryption or
write failures carry `encrypted request write`. Existing fixed stage labels,
header/body read attribution, receiver read timeout, numeric explicit RTSP
rejection, and unsupported timing/profile errors remain. No request values,
plaintext, authentication material or keys are added to logging.

Allowlist, pinned identity/endpoint, HAP signature verification, encrypted-only
control, no re-pair/retry/fallback, and bounded cleanup are unchanged. OwnTone's
pairing-key clearing on startup failure is explicitly not adopted.

Regression coverage: independently generated Python plistlib/cryptography initial
SETUP fixture checks complete encrypted request bytes (URI, headers, content
length, binary body, frame length/AAD/tag/counter). Loopback sessions compare the
production plist shape with that fixture and assert identifier stability. New
construction/write failure tests and explicit PTP response rejection complement
existing timeout, RTSP rejection, malformed response and cleanup tests. All
fixture identities and shared secrets are synthetic; no live credentials used.

A PTP-only receiver or buffered type 103 requirement remains unsupported. Feature
validation does not prove acceptance of NTP + ALAC + realtime UDP. If this change
still times out, record the safe stage/status/count diagnostics and investigate
further; do not infer PTP-only support from silence, automatically re-pair, or
claim READY. No hardware success is claimed here.

## Safe Raspberry Pi verification (operator only)

1. Build this branch with `cargo build --release --locked`. Stop any PiAir test
   invocation first; do not modify OwnTone, Autostream or system services.
2. Arrange an explicit maintenance window with the TV owner. Ensure playback is
   idle: stream-test can take over playback and the low test volume can persist.
   Do not run during other playback.
3. Keep the existing config, allowlist, selection and credentials. Confirm the
   selected/pinned receiver is `device:<canonical-id>` (Example Apple TV). If identity,
   endpoint or authorization validation fails, stop and review; do not re-pair
   or bypass checks as part of this test.
4. From an interactive terminal run once:
   `target/release/piair --config <existing-config.toml> stream-test 'device:<canonical-id>' --seconds 1`.
   Read the volume/takeover prompt and confirm the exact receiver ID only when
   authorized. This sends the existing bounded low-level test signal.
5. Record only stage/method/status, READY/failure, packet/timing counts, teardown
   outcome and physical audibility. No packet/plaintext dumps, headers, pairing
   files, PINs, keys or credentials. Zero packets before READY is expected on
   negotiation failure. Ctrl+C requests bounded cleanup; do not loop retries.
6. Report whether initial SETUP advanced and whether NTP/event synchronization
   and audible playback succeeded separately. A loopback pass proves no Apple TV
   compatibility. Restore volume manually with the owner's agreement if needed.

## Validation results

- `cargo fmt --check`: passed.
- `cargo clippy --all-targets --locked -- -D warnings`: passed.
- `cargo test --locked`: passed, 94 unit tests + 11 CLI integration tests.
- `git diff --check`: passed.

Environment: Rust 1.99.1; system libasound.so.2 via a temporary local pkg-config
entry (the image lacks ALSA development metadata). Initial sandbox test attempt
could not create loopback sockets; the complete suite passed outside that socket
sandbox. No production receiver sockets or service changes were involved.
