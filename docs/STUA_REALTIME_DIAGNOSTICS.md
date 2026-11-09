# Apple TV Stua: stored HAP credentials and realtime diagnostics

This change starts from fresh main `d8da1b7` (merged PR #36). It adds diagnostics
only for Stua `device:02d899c1833f`, without a second playback engine.
No hardware was contacted during development. Audible playback is unverified.
The operator reports PR #36 HomePod authentication, NTP, RECORD, stream SETUP,
volume, 376 transmitted RTP packets and successful TEARDOWN; that evidence does
not establish audible output or Apple TV acceptance of this profile.

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
These functions were inspected for this change. The 64-byte transient SRP secret
still uses its existing first-32-byte media key. Control/event HKDF labels,
directional counters, pairing wire envelopes and existing HomePod gates remain.

The Stua entry point authenticates via existing persistent HAP, then uses the
same `negotiate`, stream offer/response parser, event worker, NTP worker, ALAC
encoder, RTP encryption, pacing, readiness, drain and cleanup as PR #36.

## Gates and limits

Both commands require the exact Stua ID, existing allowlist approval, valid stored
Stua HAP credentials and matching advertised `pi`, and interactive terminal
confirmation. Stua does **not** need to be the globally selected default. The
commands never read, repair or overwrite the persisted selection: another
receiver may remain selected, or the selection may be absent.

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

Passive discovery supplies Stua's current literal addresses, port, capabilities
and advertised pairing ID. The method is checked against advertised persistent
HAP capability/policy, audio and ALAC; conflicting identities/endpoints fail.
These values form a temporary operation-scoped selection that is pinned for
confirmation and connection, and discarded afterward. Discovery changes never
redirect a live connection. Other advertised receivers are used only to reject
endpoint conflicts and are never contacted.

Wire now distinguishes persisted-selection authorization from Stua operation
authorization. All existing operations retain exact persisted-selection equality
checks. A Stua operation instead binds its immutable identity/endpoint selection
to the validated stored credential snapshot. The snapshot is parsed and
fingerprinted from the same protected read; before connection it must match the
credential snapshot held across confirmation. Every control/event/media/timing lease
rechecks allowlist approval, file protections and the exact credential-file
fingerprint. Removal, replacement, corruption, pairing-ID changes or approval
revocation stop further sends. Raw-byte fingerprint checks avoid repeated
TOML/key parsing on the sample timeline. Changing the unrelated default receiver
neither authorizes nor redirects Stua. No pairing, selection/credential writes,
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

Stua's default HomePod 2 output is operator context. No output routing, group
management, HomePod connection or production configuration change is performed.
Whether tvOS accepts NTP and routes this stream to that output requires physical
verification.

`stua-stream-setup-check` authenticates, performs initial SETUP, event connection,
RECORD and stream SETUP, then TEARDOWN. It generates no RTP or sync packets,
sends no SET_PARAMETER, and never opens the media sending path. NTP responses
and encrypted control/event traffic are expected; “zero audio” does not mean
zero network traffic.

`stua-playback-check` is a separate explicit gate. It requires acknowledgement of
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

## Raspberry Pi commands

Use a separate checkout; do not overwrite a deployed binary. Use the existing
config path and adjacent `receiver-state`, under its existing owner. Do not copy
secrets or select/pair/unpair anything. Keep the currently selected default
receiver as it is; Stua is authorized only for this operation using its existing
credentials and current passive discovery endpoint.

```sh
git clone --branch feat/stua-realtime-diagnostics --single-branch \
  https://github.com/kyrre-martila/PiAir.git PiAir-pr37
cd PiAir-pr37
git rev-parse HEAD # compare against the reviewed PR head
cargo build --release --locked
CONFIG=/absolute/path/to/existing/piair.toml
./target/release/piair --config "$CONFIG" config check
./target/release/piair --config "$CONFIG" receivers
umask 077
```

Zero-audio diagnostic, from an interactive terminal, for one deliberate attempt:

```sh
./target/release/piair --config "$CONFIG" -v stua-stream-setup-check \
  device:02d899c1833f 2>piair-pr37-stua-setup.jsonl
result=$?
printf 'Stua SETUP-only exit=%s\n' "$result"
```

Read the receiver and operation before answering `yes`; Enter/no/EOF cancels.
Record commit, receiver metadata, authentication, capabilities, timing profile,
initial SETUP/event/RECORD/stream SETUP results, zero audio packets, absence of
volume command, TEARDOWN acknowledgement and local cleanup. Stop after the
attempt and review the outcome. Do not infer sound from negotiation success.

Playback is separately authorized by its own prompt. Be physically present to
check Stua and its default HomePod 2 output, and ready to cancel. Receiver volume
**may remain at the selected volume after the test**. After reviewing the SETUP-only outcome, run once:

```sh
./target/release/piair --config "$CONFIG" -v stua-playback-check \
  device:02d899c1833f 2>piair-pr37-stua-playback.jsonl
result=$?
printf 'Stua playback diagnostic exit=%s\n' "$result"
```

Optional `--volume-db <integer>` selects receiver volume from **-40 through -10 dB**
inclusive; the default remains **-40 dB**. For example:
`piair stua-playback-check device:02d899c1833f --volume-db -20`.
The confirmation displays the selected volume and warns it may persist after the
test, including after failure. Only receiver volume changes; generated signal
amplitude, tone duration, silent drain and packet budget are unchanged.

Do not add duration flags or retry automatically. Record heard/not heard
separately from packet count, timing replies and TEARDOWN/cleanup. These commands
do not stop any other audio source or guarantee availability of the receiver.

## Automated verification versus hardware

Loopback tests perform real persistent HAP pair-verify, encrypted RTSP/events,
NTP, both SETUPs and RECORD through the shared diagnostic engine. They compare
stored state byte-for-byte, check the verify-derived offered media key, assert
zero audio/volume operations for SETUP-only, and decrypt/decode the fixed faded
tone and silent drain for playback. Focused credential/CLI/endpoint gates reject
missing/mismatched credentials, transient authentication, wrong IDs, endpoint
drift, conflicts and unfixed options. Failure cases cover unsupported timing,
event refusal, malformed/rejected stream SETUP, cancellation/revocation and
failed/timed-out TEARDOWN with local port reuse. Both Stua paths run with another
approved, paired receiver still selected, prove byte-for-byte preservation of
stored state, and assert that its listening endpoint receives no connection.
Operation lease tests also cover absent/default selection independence,
credential replacement/removal/schema errors, approval revocation and endpoint
drift. Existing HomePod and persisted-selection regression tests remain.

Automated checks do not establish tvOS acceptance, default-output routing,
receiver acoustics or audible playback. No physical Stua verification has been
performed as part of this PR.

Validation in the development workspace (x86_64 Linux): `cargo fmt` and
`cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`,
`RUST_TEST_THREADS=1 cargo test --locked` (**198 unit tests + 20 CLI tests
passed**) and `cargo build --release --locked` passed. Cargo.lock is unchanged.
A concurrent test run passed all new Stua cases but tripped an existing transient
playback test's 100 ms scheduler guard. The full serial rerun avoids competing
realtime tests; no scheduler limits or assertions were weakened. Raspberry Pi
compilation and physical receiver verification remain separate operator steps.
