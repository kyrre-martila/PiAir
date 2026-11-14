# Receiver pairing and credentials

Use `piair setup` for normal configuration. Manual enrollment uses explicit
`select` and `pair`; `unpair` removes local credentials only. Persistent HAP
requires a genuine physical PIN or independently obtained setup code. Transient
authentication is described in [TRANSIENT_AUTHENTICATION.md](TRANSIENT_AUTHENTICATION.md).

## Identity and approval boundary

The authorization identity is still the exact canonical
`device:<12 lowercase hex digits>`. Display names, hosts, IPs and unverified
service IDs cannot replace it. Approval is explicit `receiver_allowlist` editing;
selection does not grant approval. All network operations require interactive
exact-ID confirmation and currently approved selection. No status/config/audio
command opens receiver connections. Discovery is passive DNS-SD, with no probing.
**Samsung The Frame and every other unapproved receiver are never contacted.**

The private `Wire` owns the sole production TCP connect, taking literal pinned
addresses only (no hostname/DNS path). It reloads current approval and persisted
selection, rejects changed/ambiguous IDs and endpoints, and makes one bounded
connection attempt. No redirects, retries, alternate addresses or unauthenticated
fallback. IPv6 link-local scope handling is unsupported. Shared-address conflicting
advertisements cause rejection. Discovery comparison permits only the operational
status changes defined below; persisted approval comparisons remain exact.

Advertised `pi` is optional, untrusted discovery metadata, pinned for change
detection. It is **not** used as an authenticated identifier or mandatory expected
M6 value. After verifying SRP, M6 AEAD and the receiver signature, PiAir enrolls the
actual case-sensitive signed identifier/key under the approved stable device ID.
Its bounded identifier policy currently accepts ASCII letters/digits/colon/dash;
other HAP UTF-8 identifier forms remain unsupported. Pair-verify requires that
exact enrolled identifier and public key, independent of advertised `pi`. A
changed signed identifier or key is an error, never credential replacement.

The initial physical PIN/code proves knowledge at the approved endpoint; it does
not cryptographically prove an mDNS MAC address or manufacturer identity. If mDNS
is spoofed during initial enrollment, user approval and the independently obtained
physical code are the trust anchor. After enrollment, stored key verification
protects against endpoint impersonation. A changed advertised `pi` requires
investigation and explicit local unpair/re-enrollment; endpoint reselection alone
cannot change the enrolled identity. Do not paste a code from an unverified host.

## Sequences, timeouts and secret lifetime

```mermaid
sequenceDiagram
    actor User
    participant CLI
    participant Boundary as Approval boundary
    participant R as Selected receiver
    User->>CLI: approve stable ID; select explicit persistent method
    User->>CLI: pair ID; type exact ID
    CLI->>Boundary: reload approval, selection and passive snapshot
    Boundary->>R: one bounded TCP connection
    opt selected pin-display only
        CLI->>R: POST /pair-pin-start
    end
    CLI->>R: /pair-setup M1 method 0
    R-->>CLI: M2 salt, B, optional zero flags
    User->>CLI: hidden physical PIN or existing setup code
    CLI->>R: M3 A + proof
    R-->>CLI: M4 receiver proof
    Note over CLI: verify proof; never treat M4 as persistent success
    CLI->>R: M5 AEAD(controller ID, key, signature)
    R-->>CLI: M6 AEAD(actual receiver ID, key, signature)
    Note over CLI: verify AEAD + signature; enroll actual signed ID, not pi
    CLI->>CLI: recheck approval/selection; save version 2 credentials
    Note over CLI,R: close socket; no audio session
```

```mermaid
sequenceDiagram
    actor User
    participant CLI
    participant R as Currently approved receiver
    User->>CLI: authenticate ID; type exact ID
    Note over CLI: current boundary, capability and discovery checks
    CLI->>R: pair-verify M1 fresh X25519 key
    R-->>CLI: M2 ephemeral key + AEAD(signed receiver ID)
    Note over CLI: require stored ID/key; reject zero DH, signature/tag failures
    CLI->>R: M3 AEAD(controller ID/signature)
    R-->>CLI: M4 success
    Note over CLI: derive directional control keys; close socket and drop keys
```

`discovered`, `approved`, `selected`, and `paired` are separate facts. `paired`
means valid stored enrollment, not current authentication/liveness. Read-only
commands never claim authenticated/connected. Persistent authentication requires
completed pair-verify; transient SRP establishes only session-scoped authentication.
Default authenticate closes after the exchange. Realtime stream-test is now implemented; audible playback requires separate physical confirmation.

Connect and each I/O are limited to 3 seconds; automated phases have 15-second
absolute deadlines, recalculated for partial I/O. User PIN think time is excluded.
Messages/headers are bounded at 8 KiB. Owning handshake APIs close the socket on
success/error. Failed wire/control framing is poisoned; no retry/reuse after
failure. Owned secret storage is zeroized; the PIN drops immediately after SRP
calculation. No payload/private-key/PIN logs or secret Debug implementations.
Complete process-memory erasure is not guaranteed: serde/bigint/allocator copies
can remain, and num-bigint SRP arithmetic is not constant-time. Proof comparisons,
AEAD and signature verification use established library verification. No security
audit or protection against hostile same-user local processes is claimed.

## Credentials, migration and partial enrollment

`receiver-state/` is separate from ordinary configuration, next to its resolved
path. Linux requires owned nonsymlink 0700 directory and regular single-link
0600 files. Reads use O_NOFOLLOW, strict parsing and a 16 KiB bound. Same-directory
temporary writes are synced; credentials use no-clobber persistence. Selection
uses atomic replacement. Parent config paths and same-user processes are trusted.
The parent directory must exist. Multiple config files in one directory share state.

Version 2 credentials distinguish the actual authenticated `pairing_id` from
optional untrusted `advertised_pi`, and bind both to the approved stable ID.
Old PR #3 version 1 credentials are rejected with explicit recovery instructions;
there is no silent reinterpretation or reset. Old selection files require
reselection. Use `unpair <id>` to remove old/corrupt local credentials, inspect
receiver-side records, then select and enroll explicitly again. This pre-merge
schema change does not silently migrate a trust decision.

**Filesystem permissions are the primary local protection; no encryption at rest.**
Do not commit or expose backups of receiver-state. Corruption/permissive state
fails closed. Inspect and repair ownership/permissions manually where appropriate.
Unpair never contacts the receiver, clears selection, or revokes the allowlist;
also remove the allowlist entry to forbid future connections.

Once M5 is attempted, the receiver may enroll the controller even if M6 is lost,
invalid or local persistence fails. Errors explain that possible partial enrollment.
Nothing is saved before successful M6 verification. Approval/selection changes
before persistence prevent saving; no automatic enrollment retry or remote cleanup
occurs. Inspect local state and remove stale controller records using receiver
controls before another attempt. File sync failure can leave an installed file;
check state rather than assuming rollback. Revocation is checked before connections
and enrollment persistence, without an instant background revocation watcher.
