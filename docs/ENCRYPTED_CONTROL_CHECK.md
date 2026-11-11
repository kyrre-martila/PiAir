> Current guidance and evidence: [receiver authentication audit and safe Pi diagnostic](RECEIVER_AUTHENTICATION.md).

Current receiver discovery, selection and command usage is documented in
[Receiver diagnostics](RECEIVER_DIAGNOSTICS.md). Receiver-specific observations
and authorizations below are historical evidence, not current CLI restrictions.
Examples use placeholders; verify the canonical ID independently.
> Later operator evidence in [PR #21](INITIAL_SETUP_DIAGNOSTICS.md) reports HomePod
> transient SRP and encrypted OPTIONS success after #20. Earlier 400/403 results
> below are historical; no confirmed audible HomePod playback is documented.

# Encrypted control verification after transient SRP

PR #20 hardware evidence supplied for this work: HomePod AudioAccessory5,1,
firmware 27.2; GET /info, SRP M1/M2 and M3/M4 returned 200, and PiAir verified
the receiver SRP proof. Encrypted OPTIONS returned a valid encrypted RTSP
403 Forbidden response. No audio playback was attempted. This supports a working
encrypted channel but does not identify why the request was rejected.

## OPTIONS header correction (PR #20)

Compared [OwnTone 29.3 `request_headers_add`](https://github.com/owntone/owntone-server/blob/29.3/src/outputs/airplay.c#L886-L935)
with both PiAir request paths. OwnTone adds CSeq, its configured User-Agent and
client name, uppercase hex Client-Instance/DACP-ID, and unsigned decimal
Active-Remote through one shared function. Its digest target for OPTIONS is `*`.
It delegates Content-Length to evrtsp. It does not add X-Apple-ProtocolVersion
in that shared function; PiAir retains its existing authenticated value of 1.
OwnTone uses its library hash and receiver-derived Active-Remote; PiAir retains
its own established sender identifiers rather than copying those values.

Previously PiAir's dedicated OPTIONS sent only CSeq, User-Agent: PiAir/0.1,
and Content-Length: 0. The general authenticated RTSP path also sent
Client-Instance, DACP-ID, Active-Remote, X-Apple-Client-Name and
X-Apple-ProtocolVersion. OPTIONS now uses that same header builder, preserving
`OPTIONS * RTSP/1.0`, an empty body, checked CSeq increment, encryption and strict
response validation. The OwnTone transient profile retains its established
AirPlay/999.0.0 User-Agent from /info and SRP through encrypted control. Other
profiles retain PiAir/0.1. The agent constant is shared with plaintext OwnTone
authentication; no new identity is introduced.

The connection retains its existing client and remote values through the secure
upgrade. The OwnTone profile loads those values from the established identity
store before /info. Persistent Apple TV pair-verify and its encrypted control
keep their agent and framing; the existing independent SETUP frame fixture
must remain byte-for-byte unchanged. No pairing cryptography, SETUP/RECORD logic,
codec or media transport changes are included. A 403 still closes the connection
and fails the check without retries or fallback.

## Historical PR #20 Raspberry Pi deployment

Run these commands **on the Raspberry Pi**, from the same shell/configuration
context used for the earlier successful transient authentication. Build a separate
checkout and retain its standalone binary; do not replace a service executable.
These commands do not modify, stop, restart or reconfigure OwnTone mini or
autostream. They do not install or change any systemd unit.

```sh
git clone --branch fix/encrypted-options-identity --single-branch \
  https://github.com/kyrre-martila/PiAir.git PiAir-pr20
cd PiAir-pr20
# Record and compare this SHA with the PR head before testing.
git rev-parse HEAD
cargo build --release --locked
install -D -m 755 target/release/piair "$HOME/.local/lib/piair-pr20/piair"
```

Run the command below from the directory/context containing your existing PiAir
configuration, or supply its existing absolute path with `--config`. Do not
create a new selection or change the running services for this check.

```sh
"$HOME/.local/lib/piair-pr20/piair" -v authenticate 'device:<canonical-id>' --experimental-owntone-transient --control-check
```

Only a successful authenticated response from the physical HomePod establishes
that this header correction resolves the reported rejection. Until then,
HomePod acceptance remains unverified; the 403 could also reflect receiver
policy or other firmware requirements. These tests establish no audio support.

## Reference audit

Protocol reference: OwnTone `f7ace58dc2aeb5305360770da37fd62095f5bb21`.
Reference sources: [control transport](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/outputs/airplay.c),
[SRP and cipher framing](https://github.com/owntone/owntone-server/blob/f7ace58dc2aeb5305360770da37fd62095f5bb21/src/pair_ap/pair_homekit.c).
The Rust implementation is independent; no OwnTone source was copied.
The PR #19 encryption audit below remains applicable; PR #20 changes the
request envelope described above, retaining its phase/status diagnostics.

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

## Historical PR #20 hardware test

After building/installing this revision, use the same configuration, allowlisted
receiver and explicit transient selection used for the successful PR #18 test.
In an interactive terminal run:

```sh
piair -v authenticate 'device:<canonical-id>' --experimental-owntone-transient --control-check
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

No HomePod was available in the PR #20 workspace; no hardware test was performed there. The later PR #21 operator report above records OPTIONS success. OPTIONS is a non-playback RTSP capability query; OwnTone
recognizes its wildcard target, but this audit does not establish acceptance by
HomePod firmware 27.2 after transient SRP. A rejection is evidence to investigate,
not permission to try SETUP or change authentication. Whether the shared
headers and retained AirPlay User-Agent suffice on this firmware remains
unverified. Transient SRP proves shared session-key possession, not a
persistent receiver identity. This diagnostic establishes no audio compatibility.

Response completion follows RTSP Content-Length and the authenticated plaintext
already buffered in the consumed frame. It cannot prove the peer will never send
additional frames later; no timing-dependent socket peek is used to make that
claim. TCP write fragmentation tests cover incremental reads but the OS may
coalesce writes; correctness must not depend on packet boundaries.

PR #20 validation uses Rust 1.99.1 on x86_64 Linux, with official Debian ALSA
headers extracted locally and the installed runtime. Formatting and Clippy
(`--all-targets --locked -- -D warnings`) pass. The full serial suite
(`cargo test --locked -- --test-threads=1`) passes all 132 unit tests and 13 CLI
integration tests. `cargo build --release --locked` also passes. Tests include
persistent pair-verify, exact encrypted SETUP
fixture compatibility, encrypted OPTIONS successes and malformed response
rejections. The initial sandbox run denied loopback socket binds with
`Operation not permitted`; the successful full run used the same code outside
that sandbox. Cargo.lock is unchanged. No hardware or Raspberry Pi cross-build
result is claimed.
