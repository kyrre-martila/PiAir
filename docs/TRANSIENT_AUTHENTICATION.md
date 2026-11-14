# Transient authentication

Explicit transient selection uses connection-scoped SRP and encrypted control.
The OwnTone-reference profile is used by the working HomePod live-audio path.
No automatic authentication fallback or persistent enrollment occurs.

## Exact implemented flow

PiAir follows OwnTone's two-request transient sequence. Both requests are HTTP/1.1
POST `/pair-setup` on the same TCP connection, octet-stream TLV8, HKP 4, numeric
Host, keep-alive and correlated CSeq. Existing bounded HTTP/RTSP response parsing
is preserved. There is **no `/pair-pin-start`, M5/M6 or `/pair-verify`**.

| Message | TLV8 fields |
|---|---|
| M1 sender | State (6)=1, Method (0)=0, Flags (19)=0x10 |
| M2 receiver | State=2, Salt (2)=1–16 bytes, PublicKey (3)=1–384 bytes; optional Flags must equal 0x10 |
| M3 sender | State=3, PublicKey=384-byte A, Proof (4)=64-byte client proof |
| M4 receiver | State=4, Proof=64-byte receiver proof |

Flags are little-endian integers of 1–4 bytes; M1 uses the one-byte 0x10 form.
Absent M2 flags are accepted as in OwnTone. Split, zero, unknown or malformed
returned flags fail. Method echoes and MFi fields are unsupported, not negotiated.
TLV fragments are contiguous 255-byte chunks; duplicate, empty, truncated,
unexpected or oversized fields and wrong states fail closed. Explicit error TLVs,
non-200 status (including 403/470), missing proof, bad proof, disconnect or timeout
close the channel. No retry, profile cycling, or method fallback occurs.

pyatv differs: it first sends `/pair-pin-start` with HKP 4, then M1/M3. Its pinned
transient client does not inspect M4's receiver proof. PiAir adopts OwnTone's
proof verification. The default path does not send pyatv's additional PIN-start request;
the authenticate-only experiment linked above opts into that preamble and the
pyatv HTTP profile for M1 and M3.
OwnTone can switch to persistent PIN pairing on 470; PiAir deliberately does not.
Receivers requiring another preamble are unsupported by this implementation.

## Cryptography and encryption

SRP-6a uses the RFC 5054 3072-bit group, generator 5, SHA-512,
username `Pair-Setup`, protocol code `3939`, fresh OS-random 32-byte private a,
and receiver salt/public B. The fixed code is a protocol parameter, not a user
password or a persistent credential. Sender A is 384-byte big-endian; receiver B may omit leading zero octets.
PiAir rejects zero/out-of-range B and zero scrambling u.

With H=SHA-512, k=H(PAD(N)||PAD(g)), x=H(INT(salt)||H(username||":"||code)),
u=H(PAD(A)||PAD(B)), S=(B-k*g^x)^(a+u*x) mod N, and K=H(S), where S uses
its minimal big-endian encoding. Client proof is
H((H(N) XOR H(g))||H(username)||INT(salt)||INT(A)||INT(B)||K); receiver proof is
H(INT(A)||client_proof||K). INT is minimal unsigned big-endian, matching OwnTone
`pair.c` H_ns/update_hash_n; u and k alone use group-width padding. The independent
fixture deliberately includes leading-zero salt/A/B encodings. Proof comparison is constant-time. There is no transient
Ed25519 identity exchange or X25519 pair-verify.

The **entire 64-byte K**, without truncation, feeds HKDF-SHA512. Each output is
32 bytes. Control salt is `Control-Salt`, with sender write info
`Control-Write-Encryption-Key` and read info `Control-Read-Encryption-Key`.
Event salt is `Events-Salt`, with sender write info `Events-Read-Encryption-Key`
and read info `Events-Write-Encryption-Key` (existing reversed event directions).
The existing ChaCha20-Poly1305 framing is unchanged: <=1024-byte blocks,
2-byte little-endian length as AAD, 4 zero nonce bytes plus a directional
little-endian u64 counter. Counters start at zero only on this newly authenticated
connection. Failed tags, exhausted counters or I/O poison/close the channel.
PR #10 sender identity headers and realtime ALAC/44100/16-bit/stereo, UDP and
NTP behavior are preserved; media keys remain freshly generated separately.

## Trust model and capability policy

The public fixed code means a valid SRP proof establishes a shared encrypted
session with a peer implementing this protocol. **It does not authenticate a
persistent receiver identity**, prove Home membership, or resist an active local
impersonator who knows the fixed code. mDNS device ID, `pi` and endpoints are
pinned authorization inputs, not cryptographic identities. Use only on a trusted
network with explicit receiver authorization. Use persistent pairing when durable
identity verification is needed. No FairPlay, MFi, password or external service
is used, and transient never activates after persistent failure.

Before connection, require feature bit 43 or 48 and present status flags. Reject
PIN-required bit 3, one-time/persistent-pairing-required bit 9, password bit 7 or
`pw=true`, nonzero `acl` or `act`, malformed/conflicting fields and unknown
protocol versions. Unknown status bits stay pinned; only the existing documented
operational-bit exceptions may drift. Feature bits are capability hints, not
proof of support: some Apple TVs reject transient (OwnTone notes ATV4/470).
Bit 51 alone is not sufficient and no MFi exchange is attempted. The supplied
HomePod status 0x98404 does not set these protection bits; its complete discovery
profile must still satisfy selection policy. Firmware/model names confer no
exception. Protected profiles are rejected, never bypassed.

Secrets and proofs use zeroizing owners. The SRP private byte buffer, premaster
bytes, K, TLV buffers and control/event keys are cleared on drop where practical.
The existing `num-bigint`/`srp` arithmetic allocates internal BigUint temporaries
that cannot promise secure erasure or constant-time arithmetic; no claim of total
process-memory erasure is made. No transient secret or pairing credential is
serialized. A consumed Wire owns keys, offers no extraction/reconnect interface,
and closes on failed authentication. Existing request/handshake deadlines and
bounded worker/teardown cleanup remain; failed sessions release owned resources.
Existing persistent credential files are neither overwritten nor removed.
