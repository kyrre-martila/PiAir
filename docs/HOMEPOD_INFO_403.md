# HomePod initial /info 403 investigation (PR #18)

Current receiver discovery, selection and command usage is documented in
[Receiver diagnostics](RECEIVER_DIAGNOSTICS.md). Receiver-specific observations
and authorizations below are historical evidence, not current CLI restrictions.
Examples use placeholders; verify the canonical ID independently.

Base: fresh `origin/main`, `c1e9def0abbbd9a312cb6c9aae1c3f7262ec1a31`.
Receiver evidence supplied by the operator: `device:<canonical-id>`,
AudioAccessory5,1, firmware 27.2; initial `GET /info RTSP/1.0` received
`RTSP/1.0 403 Forbidden`. No packet capture, numeric peer endpoint or complete
hardware log was supplied. No hardware requests were made in this investigation.

## Supported cause and narrow fix

PiAir's experimental OwnTone profile sent `User-Agent: PiAir/0.1`. The pinned
OwnTone revision actually defaults to `AirPlay/999.0.0`. This difference was
incorrectly treated as an innocuous sender identity in the earlier analysis.

At [pinned conffile.c:173–182](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/conffile.c#L173),
`airplay_shared.user_agent` is explicitly accompanied by the comment
“HomePod OS 27 has user-agent validation, see issue #2042”.
[airplay.c:4314–4315](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c#L4314)
loads that configuration, separately from the library/client name.
[Upstream fix 34603ba998bbbe3f3fbb04c74fde4a0a39e0fd79](https://github.com/owntone/owntone-server/commit/34603ba998bbbe3f3fbb04c74fde4a0a39e0fd79)
is an ancestor of the pin and specifically fixes HomePod OS 27 rejection.
[Issue #2042](https://github.com/owntone/owntone-server/issues/2042)
contains reports of `/info` 403 with nonaccepted agents, 200 with
`AirPlay/380.20.1`, and confirmation that `AirPlay/999.0.0` works.
Those are upstream operator observations, including OS 27.0 devices, not a
controlled retest of this operator's firmware 27.2 receiver.

The fix uses exactly the pinned default for GET and both pair-setup requests
in the explicitly selected OwnTone experiment. PiAir's client name and independent
public identifiers remain its own. No configurable sweep, fallback or extra
attempt is added. Other profiles retain their existing wire contracts.
This concrete mismatch plausibly explains the supplied 403; resolution on this
HomePod and subsequent authentication remain unverified.

## Reference trace, at f7ace58dc2aeb5305360770da37fd62095f5bb21

All source references below are relative to
[that exact tree](https://github.com/owntone/owntone-server/tree/f7ace58dc2aeb5305360770da37fd62095f5bb21).

1. `src/outputs/airplay.c:4348` browses `_airplay._tcp` with
   `MDNS_CONNECTION_TEST`. `airplay_device_cb` (3933 onward) parses TXT `deviceid`
   and features and records the resolved family-specific numeric address and
   **the advertised SRV port** (4113–4127). It does not substitute a RAOP port.
   The separate legacy output browses `_raop._tcp` (`raop.c:4718`). The AirPlay
   output can defer unsupported devices to RAOP; PiAir deliberately has no such
   fallback. Neither path selects HomeKit, companion or other service ports.
2. In the Avahi backend, `src/mdns_avahi.c:579–681,695–727` tests TCP
   connectability and closes the test socket; it sends no application request.
   IPv6 configuration (disabled by default in `conffile.c:57`) and link-local
   filtering affect discovery. This is a
   separate connection, not an authentication preamble or established session.
3. `session_make` (`airplay.c:1585–1640`) initializes CSeq=1 and tries IPv6
   connection setup before IPv4. `session_connection_setup` (1359–1436) takes
   the selected family's advertised address/port, handles IPv6 interface scope,
   and creates `evrtsp_connection_new`. TCP connects when the request is queued
   (`src/evrtsp/rtsp.c:1423–1500`); plain RTSP, no TLS handshake or HTTP upgrade.
4. Both start and probe sequences begin with GET `/info` (`airplay.c:3643,3659`,
   entry points 4150–4177). No OPTIONS, PIN-start, auth-setup, pair-setup or
   pair-verify precedes it on this socket. `/auth-setup` belongs to the later
   playback sequence. Policy in the `/info` response selects later pairing.
5. `request_headers_add` (886–935), `sequence_continue` (3775–3824), and
   `evrtsp_make_header_request`/`evrtsp_make_header` (`rtsp.c:327–371`) produce
   the following ordered CRLF lines, followed by one empty CRLF line:

```text
GET /info RTSP/1.0
CSeq: 1
User-Agent: AirPlay/999.0.0
X-Apple-Client-Name: <configured library name>
Client-Instance: <libhash, uppercase unpadded hex>
DACP-ID: <same libhash>
Active-Remote: <low 32 bits of receiver device ID, decimal>
```

The library name defaults to `My Music on %h` (`conffile.c:86`), expanded
with the host name; `libhash` is its Murmur64 hash (`conffile.c:411,495`).
The User-Agent can be configured in OwnTone; this is its pinned **default**,
not proof of a particular running OwnTone instance's configuration. Fresh
sessions have no digest realm/nonce, hence no Authorization. `AIRPLAY_USE_STREAMID`
is 0 (`airplay.c:71`). GET has no payload maker or content type; evrtsp appends
Content-Length only for nonempty bodies. No Host, Connection, HKP,
X-Apple-ProtocolVersion, body, or absolute RTSP session URI is synthesized here.
The version defaults to 1.0 (`rtsp.c:1473–1476`); header insertion preserves
order (`TAILQ_INSERT_TAIL`, 1033).

## PiAir comparison and service verification

`discovery.rs` passively resolves only `_airplay._tcp.local.` and
`_raop._tcp.local.`; separate advertisements retain their own addresses/ports.
`Selection::from_receiver` (`receiver/mod.rs`) requires exactly one AirPlay
service matching the stable device ID, validates policy, and pins that service's
address set and advertised port. A same-IP RAOP record cannot supply the port.
Missing/duplicate AirPlay endpoints fail closed. Other services are ignored.

`Wire::connect` rechecks allowlist, persisted selection, identity, policy and
endpoint immediately before connecting. It chooses the first IPv4 in the ordered
address set, otherwise the first IPv6, and connects once to the pinned port.
That family preference differs from OwnTone; there is no evidence it explains
an application-level 403. No numeric port (including 7000) is hardcoded. The
supplied hardware report cannot prove the actual advertised or connected IP/port;
the new debug endpoint event makes that observable. It logs only the validated
AirPlay service type and chosen numeric peer, never TXT public keys or pairing data.

`owntone_transient` → `owntone_exchange` → `owntone_header` writes the complete
header string directly to the socket, then an empty body. Before this fix it
matched the reference layout above except for the agent, PiAir client name,
independent persisted sender Client-Instance/DACP-ID and independent decimal
Active-Remote. No extra headers are inserted by a library. After the fix the
agent matches too; the public identifier differences are intentional, and no
evidence establishes they cause rejection. TCP write segmentation is not an
RTSP message boundary.

Diagnostic `headers` is a **filtered value allowlist**, not a wire capture:
X-Apple-Client-Name, Client-Instance, DACP-ID and Active-Remote were transmitted
but omitted from logs. `headers_filtered=true` now makes that limitation explicit;
the exact new User-Agent is allowlisted. Bodies, proofs, cookies, credentials and
arbitrary receiver values remain excluded. Deterministic loopback tests inspect
all received bytes independently of logging and the header test helper, advertise
RAOP first on a different port, require the AirPlay connection, and confirm a 403
closes without further requests or connections. Existing tests cover M1/M3 bytes,
proof validation and failure handling.

## Hardware retest (operator only)

Keep existing approval, selection and pairing files. Review the physical receiver
and its current AirPlay advertisement before one explicitly authorized attempt:

```sh
piair --config config.toml -v authenticate 'device:<canonical-id>' --experimental-owntone-transient
```

Type the exact receiver ID at the interactive prompt. Do not run stream-test,
change Home access settings, unpair, or add a preamble. If endpoint drift is
reported, inspect the discovery evidence before any explicit reselection; do not
rewrite saved state automatically. Record the validated `_airplay._tcp.local.`
peer IP/port locally and compare it with that receiver's current AirPlay SRV
record, independently of its RAOP/other records. Publish only necessary endpoint
metadata; redact private addresses if sharing publicly.

Verify the request event shows `AirPlay/999.0.0` and the initial response changes
from 403 to 200. Continue only through the invocation's existing policy and SRP
checks; success requires verified M4 proof and channel initialization. A 200
alone proves neither authentication nor audio compatibility. Any rejection ends
this attempt without retry/fallback. Report model/firmware, phase/status and
whether the validated peer matches AirPlay discovery. Never share pairing
payloads, persistent files or packet captures containing authentication material.
`--control-check` is optional only if explicitly desired; it is unnecessary to
resolve the initial `/info` result. No playback or persistent pairing mutation
is part of this fix or retest.

## Validation

`cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`,
`cargo test --locked --quiet` (129 unit tests, 13 integration tests), and
`git diff --check` passed. Rust 1.99.1; the installed ALSA runtime was linked
using a workspace-local pkg-config descriptor. The sandbox disallowed socket
creation, so loopback tests ran outside it. Cargo.lock is unchanged. These
checks use synthetic receivers and do not establish HomePod compatibility.
