# Stored HAP realtime diagnostics

These commands accept any allowlisted compatible receiver with valid stored
HAP credentials, chosen by canonical ID or the existing interactive picker.
The old `stua-stream-setup-check` and `stua-playback-check` names remain aliases.
See [receiver discovery, selection and usage](RECEIVER_DIAGNOSTICS.md).
No physical receiver contact or live playback was performed for this change.

## Authentication and key material

The protocol conclusion comes from the implementation and stored schema, not
from the on-screen PIN. `store::Credential` version 2 holds a controller Ed25519
seed/ID and pinned receiver Ed25519 public key/authenticated pairing ID, bound to
the stable receiver ID and advertised `pi`. `PinDisplay` and `SetupCode` enroll
persistent HAP method 0; both authenticate existing credentials with the same
HAP pair-verify M1–M4 implementation. `Transient` instead uses session SRP and
cannot use these credentials. Legacy/FairPlay/Companion credentials are not
accepted, converted or inferred. No PIN is requested by either new command.

Pair-verify generates fresh X25519 material, decrypts and checks M2's pinned
receiver identity/signature, signs the controller proof and requires successful
M4. The resulting 32-byte X25519 secret already supplies the control and event
HKDFs. Previously it was discarded after those derivations. It now also supplies
the realtime media key, installed only after M4 and authorization recheck, owned
by the same Wire and consumed into the existing zeroizing Encoder. No raw key is
logged or persisted. Authentication failure yields no usable media session.

This matches pinned OwnTone
[`f7ace58`](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c):
`response_handler_pair_verify2` near L3548 passes the verify result to
`session_cipher_setup` near L1438, which uses the first 32 bytes for packet
ciphering. Its stream offer carries that key as `shk` inside encrypted SETUP.
These functions informed the original implementation. The 64-byte transient SRP secret
still uses its existing first-32-byte media key. Control/event HKDF labels,
directional counters, pairing wire envelopes and existing protocol gates remain.

The HAP diagnostic entry point authenticates via existing persistent HAP, then uses the
same `negotiate`, stream offer/response parser, event worker, NTP worker, ALAC
encoder, RTP encryption, pacing, readiness, drain and cleanup as PR #36.

## Gates and limits

Both commands require an independently verified canonical receiver ID, existing
allowlist approval, valid stored HAP credentials and matching advertised `pi`,
and terminal confirmation. With an explicit ID, the default selection is left
unchanged. Omitting the ID reuses the existing receiver/method picker, and final
confirmation saves its provisional persistent selection and endpoint pin. The
operation derives authentication-only Stored HAP from the validated credentials.

The trusted credential schema v2 is emitted exclusively by PiAir's signed
persistent HAP method-0 enrollment. It unambiguously establishes the HAP
pair-verify authentication method. It does not store the historical PIN delivery
choice (`PinDisplay` versus `SetupCode`); that choice is unnecessary for
pair-verify and is never guessed from receiver name, model, PIN behavior or
current default selection. A validated v2 credential selects the explicit
internal `StoredHap` authentication-only method. Unknown/legacy schema versions,
invalid identities/keys or unestablished protocols fail closed before contact.
`StoredHap` cannot be chosen for enrollment or serialized as a default selection.
Existing credential files require no migration, added fields or writes.

Passive discovery supplies the chosen receiver's current literal addresses, port, capabilities
and advertised pairing ID. The method is checked against advertised persistent
HAP capability/policy, audio and ALAC; conflicting identities/endpoints fail.
These values form a temporary operation-scoped selection that is pinned for
confirmation and connection, and discarded afterward. Discovery changes never
redirect a live connection. Other advertised receivers are used only to reject
endpoint conflicts and are never contacted.

Wire now distinguishes persisted-selection authorization from HAP diagnostic operation
authorization. All existing operations retain exact persisted-selection equality
checks. A HAP diagnostic operation instead binds its immutable identity/endpoint selection
to the validated stored credential snapshot. The snapshot is parsed and
fingerprinted from the same protected read; before connection it must match the
credential snapshot held across confirmation. Every control/event/media/timing lease
rechecks allowlist approval, file protections and the exact credential-file
fingerprint. Removal, replacement, corruption, pairing-ID changes or approval
revocation stop further sends. Raw-byte fingerprint checks avoid repeated
TOML/key parsing on the sample timeline. Changing the unrelated default receiver
neither authorizes nor redirects the operation. No pairing, credential writes,
method fallback, reconnect or automatic retry exists. The shared public sender
identity file can be initialized if absent; pairing and default selection data
are preserved.

Only audio+ALAC, realtime UDP type 96, 44100 Hz/16-bit/stereo, 352 frames/packet
and NTP are implemented. Encrypted `/info` validates codec capabilities; initial
SETUP explicitly offers NTP and rejects a returned unsupported timing protocol;
stream SETUP validates the fixed profile and negotiated ports. Advertising PTP
capability alone does not establish that NTP is forbidden. PTP-only, buffered103,
MFi/FairPlay/password/Home authorization requirements have no fallback. Earlier
Apple TV initial SETUP failures remain relevant: stored credential success does
not establish transport compatibility.

`hap-stream-setup-check` authenticates, performs initial SETUP, event connection,
RECORD and stream SETUP, then TEARDOWN. It generates no RTP or sync packets,
sends no SET_PARAMETER, and never opens the media sending path. NTP responses
and encrypted control/event traffic are expected; “zero audio” does not mean
zero network traffic.

`hap-playback-check` is a separate explicit gate. It requires acknowledgement of
the selected receiver volume (**-40 dB** by default), then timing readiness.
Volume can persist even when later work fails and is not restored. The existing original faded triangle tone
has one second of content, peak below -18 dBFS and 50 ms fades, followed by two
seconds silent drain. Success sends the existing fixed 376-packet budget,
including silent packet padding. There are no retransmissions or automatic
retries. Ctrl+C cancels; scheduler, timing and worker failures stop transmission.

After a remote SETUP attempt, every exit attempts TEARDOWN when the authenticated
transport is still valid and authorized, then releases sockets, joins workers
and drops keys. TEARDOWN uses the existing bounded request budget (15 seconds normally,
250 ms during failure cleanup).
Revoked/invalidated transport prohibits further sends: a skipped/failed TEARDOWN
is not acknowledged and remote resource expiry is uncertain. Local cleanup still
runs. Existing HomePod diagnostic behavior and all legacy flags/files remain.
OwnTone, autostream, services and deployed configurations are untouched.
