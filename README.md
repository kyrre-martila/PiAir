# PiAir

A lightweight, headless AirPlay 2 audio transmitter project for Raspberry Pi OS
64-bit (Debian ARM64), written in Rust. **This first milestone provides local
configuration and passive receiver discovery only. AirPlay playback is not
implemented.** No audio device or existing Autostream/OwnTone service is touched.

## Build and use

Install a current stable Rust toolchain with rustfmt and Clippy. On Raspberry Pi
OS 64-bit, install the ordinary C build toolchain (`build-essential`, if missing),
then build natively without root:

```sh
cargo build --release
./target/release/piair --help
./target/release/piair --version
./target/release/piair status
./target/release/piair config check
./target/release/piair config show
./target/release/piair --config config.example.toml devices
./target/release/piair -v devices
```

For cross-compiling on Debian x86_64, install `gcc-aarch64-linux-gnu`, add the
Rust target, and select its linker:

```sh
rustup target add aarch64-unknown-linux-gnu
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc cargo build --release --target aarch64-unknown-linux-gnu
```

The cross-built binary is `target/aarch64-unknown-linux-gnu/release/piair`.
Use a sysroot/toolchain compatible with the destination Debian glibc version.
No ALSA development libraries, Docker, root execution, service installation, or
privileged port binding are needed for this milestone. `devices` requires a LAN
interface with multicast DNS access (UDP 5353). Network isolation/firewalls can
produce an empty snapshot; this is not proof that no receivers exist.

## Configuration

Resolution order is `--config PATH`, then `PIAIR_CONFIG`, then
`$XDG_CONFIG_HOME/piair/config.toml`, otherwise `$HOME/.config/piair/config.toml`.
XDG_CONFIG_HOME must be absolute (empty counts as unset). HOME must be absolute
when used. Explicit CLI/environment paths may be relative to the working
directory. No system-wide file is implicitly read. Only an absent default file
uses built-in defaults; a missing explicit file, unreadable file, malformed TOML,
unknown key, wrong type, invalid timeout, or invalid/duplicate allowlist entry
fails with exit code 1. Help/version do not read config or access the network.
Read-only commands never create a config or directory.

Copy `config.example.toml` to the resolved user path yourself if persistence is
desired. `config show` prints validated effective TOML; `config check` identifies
the source. Defaults are a five-second discovery window and an empty allowlist.
Timeouts are integers in 1..60 seconds, plus at most one second for shutdown
acknowledgment under normal scheduling. Structured JSON logs go to stderr;
`-v` enables debug and `-vv` trace. No raw TXT or key material is logged.

`status` reports this invocation's local readiness, loaded/default configuration,
validation, allowlist count, and unimplemented audio/session components. It does
not inspect a running daemon or invent streaming/receiver status.

## Discovery and approval

`devices` browses `_airplay._tcp` and `_raop._tcp` using multicast DNS only and
exits after a bounded window. It never uses RTSP, HTTP `/info`, pairing,
authentication, audio streaming, or direct receiver connections. Output retains
name, service instance/type, IPs, port and recognized advertised capabilities
(feature flags, model, sample formats/rates, encryption/transport flags, when
available). Values remain unverified advertisements; discovering AirPlay does
not establish AirPlay 2 compatibility. Missing/malformed fields are tolerated.

Records merge only through a matching normalized device identifier. They never
merge solely by name, IP, or hostname. Conflicting identifiers remain unmerged.
Valid IDs appear as `device:aabbccddeeff`; ID-less entries use service-scoped
fallback IDs and cannot be approved. Copy a canonical device ID to
`receiver_allowlist` only after verifying the intended receiver. Duplicate IDs,
noncanonical IDs, and zero/broadcast IDs are rejected. All receivers absent from
the allowlist are clearly marked `UNAPPROVED`; listing does not approve/select one.

**Unknown receivers must never be contacted automatically.** Future connection
code must enforce the allowlist at the connection boundary, including reconnect,
before any direct receiver communication. Filtering CLI output is insufficient.
Advertisements and their IDs are unauthenticated; approval is not proof of identity.

## Design and implementation phases

See [architecture](docs/ARCHITECTURE.md) and the binding future
[audio activity specification](docs/AUDIO_ACTIVITY.md).

| Phase | Deliverable | Measurable acceptance criteria |
| --- | --- | --- |
| 1 (this PR) | CLI, validated config, passive discovery | Six requested CLI forms work; invalid config exits nonzero; device-free tests cover validation/allowlisting/TXT/merging; fmt, Clippy and tests pass on Linux; multicast-only packet capture on target LAN; no receiver selected/contacted |
| 2 | ALSA capture, PCM meter, confirmed calibration, detector, pre-roll | Synthetic dBFS/FSM tests pass; monitor/calibration never contact receivers; measured hardware noise cannot sustain playback intent; leading-edge retention measured |
| 3 | Explicit approved AirPlay 2 session and transport | Independent protocol implementation; authentication/crypto/dependency review; connection-boundary deny tests, including unknown receivers and reconnect; real approved receiver interoperability measured |
| 4 | Activity-driven playback and one systemd service | Actual session ends after stop duration plus bounded teardown; bounded recovery and queues; capture/network lifecycles independent; ARM64 soak/coexistence tests; no changes to existing installations |

Phase 1's LAN packet capture and ARM64 execution require real hardware/network
validation; unit tests do not establish those results. Later phases must not
claim support until their acceptance tests pass. No systemd installation or
session initiation belongs in this PR.

## Development and licensing

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Linux CI runs these independently; tests need no Raspberry Pi, AirPlay device,
root, or live multicast traffic. The application forbids unsafe code. Keep the
architecture minimal: one executable and eventually one service, no web UI,
database, Bluetooth, Spotify or media library. Preserve existing licenses/notices.
Original code is Apache-2.0. No source from Autostream, OwnTone/OwnTone Mini or
airplay2-rs is imported, translated or copied; those projects may only be technical
references. Review the resolved dependency licenses and notices before release.
The initial dependency resolution/lockfile must be recorded in a Rust-capable
environment before producing a reproducible release.
