# Encrypted control verification after transient SRP

PR #18 hardware evidence supplied for this work: HomePod AudioAccessory5,1,
firmware 27.2; GET /info, SRP M1/M2 and M3/M4 returned 200, and PiAir verified
the receiver SRP proof. No audio session was established. That evidence does
not establish that the subsequent encrypted control request succeeds.

## Reference audit

Protocol reference: OwnTone `f7ace58dc2aeb5305360770da37fd62095f5bb21`.
Reference sources: [control transport](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c),
[SRP and cipher framing](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/pair_ap/pair_homekit.c).
The Rust implementation is independent; no OwnTone source was copied.
The existing `--control-check` is retained, with phase/status diagnostics added.

| Concern | Pinned OwnTone reference | PiAir implementation and result |
| --- | --- | --- |
| Transient completion | `src/outputs/airplay.c`, `response_handler_pair_setup2`, obtains setup result then initializes cipher | `pairing::transient` verifies M4 proof before upgrading the owned Wire; no M5/M6 or persistent enrollment |
| Secret input | `session_cipher_setup` passes the entire transient shared secret and length to control cipher | `srp::Client::finish` returns the 64-byte SHA-512 SRP session key; the entire key enters `ControlKeys::new` |
| Derivation | `src/pair_ap/pair_homekit.c`, `hkdf_extract_expand`, `pair_keys_map`, `cipher_new` | HKDF-SHA512, 32-byte keys, `Control-Salt`; client sends with `Control-Write-Encryption-Key`, receives with `Control-Read-Encryption-Key` |
| Initialization | `cipher_new` zero initializes separate encryption/decryption counters | Both counters start at zero only after proof verification; plaintext /info and SRP exchanges consume neither counter |
| Connection | `session_cipher_setup` installs cipher callback on existing `session->ctrl` | `Wire::secure` upgrades the existing authenticated TCP socket; no reconnect |
| Framing | `encrypt`/`decrypt`: maximum 1024-byte blocks, 2-byte length authenticated as AAD, 16-byte tag, 12-byte nonce with counter after four zero bytes | Existing RustCrypto ChaCha20-Poly1305 composition matches; explicit little-endian length and u64 counter, increment per authenticated frame, independently in each direction |
| Request target | `src/outputs/airplay.c`, `request_headers_add`, uses `*` for OPTIONS digest target | `Wire::options` sends `OPTIONS * RTSP/1.0`, empty body, next connection CSeq (4 after OwnTone /info + M1 + M3) |
| Reassembly | RTSP cipher callback consumes complete encrypted blocks and leaves incomplete input buffered | `Wire::read_exact` reads complete length/ciphertext/tag before decryption; pending plaintext spans RTSP reads and frame boundaries |
| Validation | Reference uses its RTSP parser and authenticated cipher callback | PiAir checks AEAD tag, frame size, RTSP/1.0, status 200, required matching CSeq, unique headers, bounded Content-Length and empty OPTIONS body; rejects extra plaintext in the final consumed frame |

OwnTone's native-integer memcpy has endianness TODOs. PiAir explicitly uses
little endian, consistent with the reference on its usual little-endian hosts
and the existing independent HAP byte fixtures. No wire-format change was needed.
PiAir intentionally fails closed rather than reproducing OwnTone's authentication
fallback, encryption rollback or playback negotiation. Existing persistent
pair-verify / Apple TV paths and cryptography are unchanged.

## Exact hardware test

After building/installing this revision, use the same configuration, allowlisted
receiver and explicit transient selection used for the successful PR #18 test.
In an interactive terminal run:

```sh
piair -v authenticate device:3ae5c9a302ad --experimental-owntone-transient --control-check
```

Confirm the exact receiver ID at the existing prompt. Substitute the exact
approved ID if testing a different receiver. This command performs /info and
transient SRP, then one encrypted OPTIONS request on the same connection.
It never calls SETUP, RECORD, volume/configuration operations, media workers,
persistent pairing, or another protocol/authentication method. Normal cleanup
closes the TCP connection without TEARDOWN because no audio setup was attempted.

Expected results:

- /info, M2 and M4 return 200 and the SRP proof verifies as in PR #18.
- Verbose diagnostics enter `encrypted OPTIONS`, report numeric status 200,
  then `encrypted control check validated` with CSeq 4.
- CLI prints `Authenticated encrypted OPTIONS completed; buffered audio remains unsupported.`
- CLI reports transient authentication, no saved credentials, connection closed,
  and no audio session; process exits successfully.

A tag/counter/framing error, malformed or non-200 response, incorrect/missing
CSeq, closure or timeout fails the command and closes the connection. There is
no silent retry or fallback. Report only phase/status, error category, model and
firmware. Logs do not include keys, proofs, plaintext secrets, decrypted headers,
response reason phrases or encrypted payloads.

## Validation and limits

Loopback tests exercise the production SRP, Wire and Session implementations
with fixed receiver-side SRP material (client ephemeral randomness remains OS
provided). The OwnTone profile tests cover /info + M1/M4 on one socket, successful
OPTIONS, multiple exchanges with asymmetric frame counts, fragmented length and
tag bytes, invalid tags, a counter starting at one, incorrect/duplicate CSeq,
403 rejection, incomplete declared body, unsolicited plaintext, and closure
before or inside a frame. Every failure asserts closed connection and failed
session; the mock accepts only OPTIONS followed by EOF, preventing audio or
fallback requests. Existing independent SRP/HKDF/framing fixtures check exact
bytes in addition to the production-code loopback tests. Existing persistent
pair-verify and CLI integration tests remain part of the full suite.

No HomePod is available in this workspace: **encrypted OPTIONS on hardware has
not been tested here**. OPTIONS is a non-playback RTSP capability query; OwnTone
recognizes its wildcard target, but this audit does not establish acceptance by
HomePod firmware 27.2 after transient SRP. A rejection is evidence to investigate,
not permission to try SETUP or change authentication. The existing PiAir OPTIONS
User-Agent remains PiAir/0.1; whether this firmware requires the /info experiment's
OwnTone identity headers for encrypted OPTIONS is unverified. Do not change them
without evidence. Transient SRP proves shared session-key possession, not a
persistent receiver identity. This diagnostic establishes no audio compatibility.

Response completion follows RTSP Content-Length and the authenticated plaintext
already buffered in the consumed frame. It cannot prove the peer will never send
additional frames later; no timing-dependent socket peek is used to make that
claim. TCP write fragmentation tests cover incremental reads but the OS may
coalesce writes; correctness must not depend on packet boundaries.

Checks for this revision use Rust 1.99.1 on x86_64 Linux, with official Debian
ALSA development headers extracted locally and the installed ALSA runtime:
`cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
`cargo test --locked --test cli`, and `cargo build --locked --release` pass.
The first full `cargo test --locked` run passed 130 unit tests and failed the
existing realtime streaming test at its 100 ms sample-timeline deadline while
builds ran concurrently. The full suite was rerun with
`cargo test --locked -- --test-threads=1` passed all 131 unit tests and 13 CLI
integration tests, including the previously failing realtime test.
Cargo.lock is unchanged. No hardware or Raspberry Pi cross-build result is claimed.
