# Authenticated PTP timing peer selection

Current receiver discovery, selection and command usage is documented in
[Receiver diagnostics](RECEIVER_DIAGNOSTICS.md). Receiver-specific observations
and authorizations below are historical evidence, not current CLI restrictions.
Examples use placeholders; verify the canonical ID independently.

The encrypted initial SETUP response may delegate timing to an address different
from the authorized RTSP endpoint. PiAir accepts only numeric addresses explicitly
listed in `timingPeerInfo.Addresses`, bound to that response's mandatory valid
`ClockID`. This does not add a receiver to the allowlist. The original receiver
selection, authentication, live allowlist and revocation checks remain mandatory
for every PTP receive/send operation.

Selection is bounded to 1–32 entries (each numeric string at most 45 bytes).
Malformed strings, DNS names, scoped strings and non-string entries fail the
response, even if another entry would match. Unsafe numeric entries are rejected
as candidates; incompatible entries are skipped, allowing mixed-family HomePod
advertisements. Exact duplicates collapse without changing order. The advertised
RTSP IP wins; otherwise the first compatible safe address in advertisement order
wins. Multiple valid addresses therefore have an explicit deterministic ordering,
not an ambiguity requiring rejection. No random ordering or inferred address is
used. There is no retry to another address after selection or after a PTP timeout.

Unspecified, multicast, loopback, IPv4 limited broadcast, 0/8, 240/4 and
IPv4-mapped IPv6 are excluded. Only unit-test loopback fixtures may use the exact
loopback RTSP address. Private IPv4, IPv4 link-local and IPv6 ULA/global unicast
are permitted subject to family and route checks. Subnet-directed broadcast is
not detected by address parsing; the connected sockets do not enable SO_BROADCAST
and any send failure closes the timing service.

The selected peer must use the RTSP connection's address family. Before opening
PTP sockets, a temporary wildcard UDP socket performs a kernel route lookup
(`connect`, no datagram sent). Its chosen local address must equal the RTSP local
address, and the timing peer must not equal that local address. Event/general
sockets bind that same local address and connect exclusively to the selected
IP on UDP 319/320. The service's capability checks the selected destination;
kernel connected-UDP filtering rejects other source IPs and ports. Existing packet
length, version, domain, message, source port identity, authenticated ClockID,
sequence and liveness checks remain in force. Existing lifecycle SETPEERS uses
the service's selected timing address too. No UDP packet or private TLV can
register another peer. Source-IP and ClockID checks are not cryptographic UDP
authentication: a network attacker capable of spoofing both is outside this
transport's protection. Even valid traffic produces only `PeerObserved`, never
clock adoption or permission to send media.

## Interface and IPv6 limitations

PiAir has no interface-index/pktinfo infrastructure. IPv6 link-local addresses
and `%zone` syntax are unsupported and fail safely (an unscoped link-local entry
can be skipped in favor of another compatible address). Cross-family timing,
multiple local interfaces, source-policy routing and interface-pinned socket
behavior require further work. The route-source check can reject a legitimate
multihomed configuration; it intentionally does not choose another local address
or silently retry a peer. It checks routing at setup time, not subsequent route
changes, and is not a SO_BINDTODEVICE guarantee. Non-link-local IPv6 uses ordinary
connected sockets. IPv6 subnet-router anycast cannot be distinguished from unicast
using just this advertisement.

## Reference comparison

Reviewed OwnTone Mini `src/outputs/airplay.c`, `handle_timingpeerinfo()` and
`response_handler_setup_session()` as behavioral reference only (reviewed file
blob `d6fbb0dcb40fe44aea2b0f4273178a541f000d64`). No GPL source
was copied. OwnTone walks advertised addresses of the active family, appends an
IPv6 interface scope, and tries registering peers until successful. PiAir instead
prefers the control IP, bounds and validates the list, requires authenticated
ClockID binding, validates the route source, and commits to one destination.
PiAir does not implement OwnTone's scoped IPv6 support, registration retries,
clock-adoption behavior or playback path. It never logs a plist payload here.

## Failure and cleanup

Address failures report `authenticated timing address validation failed`;
interface failures identify the route/interface constraint. These occur before
PTP observation and are distinct from the existing PTP synchronization deadline
and unverified clock-adoption blocker. SETUP is marked attempted before sending,
so validation or socket attachment failure still enters bounded encrypted
TEARDOWN cleanup. Cancellation, revocation, transport invalidation, worker
ownership and timeout policies are unchanged. No new RECORD, playback, persistent
pairing, fallback or receiver command is introduced.

## Hardware verification (not performed)

On the authorized Raspberry Pi installation, build the release binary, retain
the existing allowlist/selection for Example (`device:<canonical-id>`, RTSP
192.0.2.119), and use only the existing initial buffered SETUP diagnostic.
Run `piair --config <config> -v buffered-test 'device:<canonical-id>' --setup-check`
only when ready to perform that authorized hardware check. The `-v` flag enables
the bounded debug diagnostics.
Check the authorized IP, advertised count, bounded validated candidate/rejection
messages, selected peer/reason and authenticated expected ClockID. Confirm both
UDP channels target only the selected advertised IP from the control local
address. Verify encrypted TEARDOWN succeeds after observation and on timing
failure/cancellation. Confirm a mismatched ClockID or different UDP source cannot
refresh timing evidence. A PTP timeout or clock-adoption blocker remains distinct
from address validation. Do not enable progression, RECORD, media, volume,
persistent pairing or fallback for this verification.
