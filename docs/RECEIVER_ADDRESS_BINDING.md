# Receiver endpoint approval and address changes

`Selection::matches_observation` in `src/receiver/mod.rs` compares the canonical
advertised ID, advertised pairing ID, explicit method, complete IP address set,
port and security-relevant capabilities. Friendly names are presentation only.
Persisted selection/revocation checks additionally retain exact saved state.

This pins the endpoints the user approved before any connection. Discovery and
`Wire::connect_authorized` reject duplicate identities/endpoints and another
identity advertising any approved address. Runtime retries a missing receiver
three times, but fails immediately on conflicts or changed bindings. It never
silently selects another receiver or endpoint. Wire chooses IPv4 first, otherwise
the first saved IPv6 address, without connection fallback.

The full set is deliberately strict. Adding/removing an IPv6 address or rotating
a temporary IPv6 address can reject a dual-stack speaker even with an unchanged
reachable IPv4 endpoint. IPv6-only changes and DHCP IPv4 changes also require
review. An intersection-only check would alter both endpoint approval and
cross-identity conflict detection. mDNS IDs and friendly names are unauthenticated;
transient SRP does not establish persistent verified receiver identity. For this
release strict binding is retained rather than broadening transient trust.

Stored HAP still requires cryptographic pair-verify of the saved credential's
receiver identity, on the same control socket; matching addresses never replaces
it. Reselection does not repair invalid credentials and never triggers transient
fallback or automatic enrollment. A malicious advertisement can still cause a
safe refusal/denial of service; strict discovery binding is not cryptographic
proof of device identity.

If PiAir reports that the selected speaker changed:

1. Stop your current playback explicitly. Confirm the intended speaker and network
   using trusted device/router settings; do not approve a guessed friendly name.
2. Run `piair setup` with the same account/config path, rescan, review the speaker
   and Advanced details, and explicitly save its connection choice. Reuse saved
   pairing when offered. Existing credentials are retained.
3. Save the reviewed playback settings, then explicitly start when ready. Confirm
   audible sound. For an advanced existing allowlisted selection, `piair select
   device:aabbccddeeff --method setup-code` is the equivalent endpoint review
   operation; replace both ID and method with your actual approved selection.

Tests cover IPv4-only, IPv6-only, dual-stack, addition/removal/rotation, changed
ports/identities and existing duplicate/conflicting discovery guards. They use
synthetic advertisements and establish no physical receiver behavior.
