# Silent Apple TV playback investigation (PR #55)

## Result and scope

The reported AppleTV6,2 / tvOS 26.6.1 test established authenticated control,
NTP replies and local UDP transmission (1504 packets / 12 seconds), **not receiver
media acceptance or audible playback**. No specific protocol defect explaining
silence is demonstrated by this source comparison. The most plausible remaining
failure boundary is receiver media decoding/acceptance or render scheduling;
there is insufficient evidence to rank those two mechanisms. An absent playback
indication is not a decoder error or proof that a command is missing.

This PR adds diagnostics and regression tests, not a speculative playback fix.
The working HomePod transient wire behavior is preserved. No receiver switching,
reauthorization shortcut, authentication fallback, new playback command, PTP,
FairPlay, metadata, or FLUSH change is introduced.

## Reproducible source scope

Inspected current upstream tips on 2026-11-11:

- PiAir main: `97669b305ad6fbbf539b5813f3cbdad475870aa1` (merged #54).
- OwnTone: `8ed84f2b3cba25d3de65e05660d59ce034b5adab`.
- pyatv: `b277a4c8222ecdcbaab8a24e3e713ca44765adb4`.

OwnTone is GPL-2.0-or-later: protocol observations only; no implementation code
copied, translated, linked or vendored. Tests and instrumentation are original
PiAir changes. Earlier analysis remains historical; the links below pin this audit.

## Evidence matrix

Confidence distinguishes source facts from causal hypotheses. “High / low cause”
means the difference is established but its ability to cause this silence is not.

| Area / source | OwnTone behavior | PiAir behavior / exact difference | Relevance and confidence | Test / action |
| --- | --- | --- | --- | --- |
| [Startup](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L4165), [PiAir](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/receiver/session.rs#L465) | Session SETUP → RECORD → stream SETUP → volume; SETPEERS skipped for NTP ([condition](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L3186)); auth-setup compile-disabled at line 73 | Same order, encrypted GET /info after verify, event TCP opened before RECORD | No demonstrated missing start operation; high for selected NTP branch | Existing encrypted lifecycle tests; do not add a guessed command |
| [URI/identifiers](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L1525), [headers](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L927), [PiAir headers](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/receiver/wire.rs#L519) | Sender IP/numeric session URI; same numeric ID for streamConnectionID; CSeq, client identity, DACP and Active-Remote | Same relevant semantics for IPv4 test. PiAir adds protocol-version header and uses PiAir client name; HAP/transient diagnostic agent follows existing profile | Cosmetic header differences lack causal evidence; high / low cause | Preserve selection, identity and pinned Session validation |
| [Event/media setup](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L3560), [PiAir](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/receiver/wire.rs#L730) | TCP eventPort; UDP dataPort/controlPort from stream response; event encryption uses pairing secret | Same endpoints on verified peer; PiAir requires worker health and an NTP exchange | Negotiation 200 doesn't prove UDP reception; high | Log first/last packet envelope; existing worker/rejection/cleanup tests |
| [Codec tuple](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L3127), [PiAir](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/receiver/profile.rs#L241) | audioFormat=262144, ct=2, sr=44100, spf=352, type=96, latencyMin=11025/max=88200, isMedia, shk, dynamic ID false | Same tuple and ID semantics; optional response echoes strictly validated | No tuple mismatch found; high | Existing typed plist tests |
| [ALAC](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L2610), [FFmpeg setup](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/transcode.c#L681), [PiAir encoder](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/audio/alac.rs#L44) | FFmpeg ALAC, 352-frame packets | Independently implemented verbatim ALAC CPE, explicit sample count; 1416-byte payload and 1452-byte encrypted UDP datagram | Concrete encoder representation difference; independent decoder proves validity, not Apple's acceptance. High / uncertain cause | New real-WAV AEAD + independent ALAC decode for every packet at 44100; hardware acceptance still needed. 48 kHz helper default is not a transmitted codec field |
| [Media key](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L1479), [PiAir](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/receiver/wire.rs#L458) | First 32 verify-secret bytes, offered as shk inside encrypted SETUP, used for ChaCha | Same for authenticated playback diagnostic, both HAP and transient | No key derivation mismatch found; high | Existing key/control tests. [pyatv](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/raop/protocols/airplayv2.py#L113) independently derives a different offered key, corroborating shk's role |
| [AEAD/RTP](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L2435), [PiAir](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/receiver/realtime.rs#L115) | Clear 12-byte RTP/PT96, first marker, sequence, sample timestamp, SSRC; timestamp+SSRC AAD; 4 zero nonce bytes + 8 transmitted bytes; tag then nonce | Same layout and random NTP SSRC ([RTP construction](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/rtp_common.c#L69)). OwnTone uses native u16 sequence nonce; PiAir increments u64 LE nonce independently | PiAir avoids upstream nonce reuse at wrap. High; no reason to weaken encryption | Existing independent encrypted fixture plus new 1504/4010-packet wrap/nonce/decoder tests. [pyatv](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/raop/protocols/airplayv2.py#L182) corroborates independent nonce counter |
| [NTP and sync](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/rtp_common.c#L309), [player stamp](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L2646), [PiAir](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/receiver/realtime.rs#L18) | d2/d3 replies and d4 sync use common monotonic clock, sync position=next+unsent−output buffer; next RTP in final word; 0x90 initially/0x80 periodically, 00 07 flags | Same field relationships with zero unsent input. PiAir wall-anchors one shared monotonic clock; initial and periodic packets map the same sample timeline | Epoch differs but replies and sync agree; no demonstrated unschedulable timestamp. High / low cause | New per-sync position, next timestamp, elapsed sample time, worst lateness, NTP reply count/age; deterministic rollover tests through 30 s |
| [Timing selection](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L4605), [PiAir](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/receiver/profile.rs#L210) | OwnTone can select PTP for capable receivers unless disabled; NTP is a separate supported branch | PiAir explicitly offers NTP and validates the response; no PTP service | Important comparison boundary: a known-audible OwnTone PTP run would not validate this NTP path. High / unknown cause | Compare on the same Apple TV with OwnTone NTP selected before inferring a missing PTP requirement; do not invent PTP fields |
| [Buffer](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L1245), [default](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/conffile.c#L79), [PiAir policy](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/receiver/realtime.rs#L15) | Default start buffer 2250 ms, sender offset 2000 ms after subtracting 250 ms | Total lead/drain 2000 ms; sender sync offset 1750 ms. Same formula, 250 ms less buffering | Concrete policy difference, both positive bounded leads; does not explain permanent silence by itself. High / uncertain cause | Preserve policy; log scheduler lateness and sync mapping before any timing experiment |
| [Resends](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L2560), [PiAir control](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/receiver/wire.rs#L1290), [diagnostic restriction](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/receiver/session.rs#L780) | d5 requests resend cached packets to data port; logs missing cache | General engine has bounded cache/retries. Playback-check engine used by WAV rejects requests, skips feedback polling; unknown control fails | Concrete diagnostic restriction. Reported success argues no parsed d5 request occurred; cannot infer absent loss or acceptance. High / unproven cause | New peer-control length/classification before validation and request seq/count/policy before rejection; retain fail-closed policy |
| [Feedback](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L4224), [pyatv polling](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/raop/protocols/airplayv2.py#L168), [PiAir](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/receiver/session.rs#L696) | Separate feedback sequence; pyatv polls every 2 s best effort | General PiAir engine polls at 2 s and validates plist; WAV/playback diagnostic skips polling | Observability gap, not evidence that polling starts playback. High / low cause | Explicitly log disabled policy. No added network operation in this PR |
| [Now-playing/UI](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L1657), [commands](https://github.com/owntone/owntone-server/blob/8ed84f2b3cba25d3de65e05660d59ce034b5adab/src/outputs/airplay.c#L4227) | Separate optional MediaRemote now-playing/playback-state sequences, capability/config gated | No metadata or MediaRemote now-playing commands | Explains a possible UI difference; does not demonstrate these commands are needed for audio. High / unproven audio cause | Do not treat no playback indication as proof of no RTP acceptance; no command added |
| [Duration](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/receiver/realtime.rs#L102), [WAV](https://github.com/kyrre-martila/PiAir/blob/97669b305ad6fbbf539b5813f3cbdad475870aa1/src/receiver/wav.rs#L25) | Long player stream, modular sequence/timestamp | The 376-packet assumption is a stale comment about initial-sequence selection, not a loop/key limit; u64 nonce never wraps with u16 RTP | Not a demonstrated silence cause. High | Correct comment; real 10/30-second asset, 1504/4010 packets with drain, forced wrap, unique nonces and decoded PCM equality |

## Diagnostics and interpretation

At debug level (`-v`), logs now expose the existing feedback/retransmission policy,
expected frame/packet counts, each successfully sent sync's NTP stamp, position and next RTP
timestamp/sequence, sample elapsed time, maximum observed scheduler lateness, and
NTP reply count/age. First and last RTP sends expose only sequence, timestamp,
marker and length. Peer-bound control messages log capped length and a fixed
classification before validation; valid resend requests log sequence/count before
the existing diagnostic rejection. No audio, decrypted receiver payload, arbitrary
header, credential, shared secret, media key or nonce is emitted.

All observations are sender-side. Silence with regular sync and no control traffic
still does not distinguish receiver packet loss, rejected codec, decryption failure,
render policy or output routing. A d5 request proves the control path is active,
not that the requested packet decoded. Missing periodic sync or excessive lateness
localizes a sender scheduling failure. Compare the initial/final RTP timestamps
and sample count, and sync offset 77175, against a known-audible OwnTone run on the
same Apple TV with NTP explicitly selected. Receiver-side logs or a separately
sanitized packet-header capture are needed to prove receiver acceptance. Capture
is not performed or enabled by this PR.

## Hardware test (operator present)

Use a reviewed release build on the already isolated test Pi, with the existing
approved, explicitly selected Apple TV and local persistent HAP credentials.
Leave production OwnTone/autostream services unchanged. With the operator ready
for sound, from the repository root run exactly one bounded test:

```sh
./target/release/piair -v stream-test device:112233445566 \
  --audio-file tests/assets/audio/Pachelbel_Canon_audio_test.wav \
  --seconds 10 --volume-db -20
```

Retain interactive confirmation. Volume may persist; no automatic restoration or
authentication fallback occurs. Expected: 1504 RTP packets over about 12 seconds
including drain; initial and periodic sync diagnostics, then TEARDOWN and cleanup.
Record audible music/onset/continuity and Apple TV indication separately from
`success=true`, along with sanitized stage logs. Ctrl-C exercises bounded cleanup.
No physical test was run by this PR, and audible playback is **not claimed fixed**.

## Validation

Validated with Rust/Cargo 1.99.1 on Linux x86_64:

- `cargo fmt --check`: pass.
- `cargo clippy --all-targets -- -D warnings`: pass.
- `cargo test`: pass, 317 unit + 6 allowlist integration + 25 offline integration
  tests (348 total), zero failures/ignored. Final run: 144.00 s / 0.17 s /
  1.01 s respectively; the preceding full run also passed (155.67 s unit suite).
- `git diff --check` and unchanged `Cargo.lock`: pass.

The initial sandboxed test invocation could not bind loopback sockets; the full
passing run used the execution environment's approved loopback access. The local
toolchain lives outside the repository. ALSA Rust bindings linked the installed
`libasound.so.2` using workspace-local pkg-config metadata; no dependencies or
build configuration were changed in the repository.

Existing encrypted loopback tests cover transient HomePod behavior, persistent
HAP authorization and selection, receiver rejection, cancellation, revocation and
cleanup. The diagnostic assertions cover both authenticated paths and prohibited
resend requests; INFO-level suppression and sanitized unknown control payloads
are tested. New duration tests run codec/crypto without real-time sleeps; they do
not emulate tvOS or establish physical HomePod/Apple TV output.
