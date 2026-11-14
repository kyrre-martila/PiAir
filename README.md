# PiAir

PiAir sends Raspberry Pi USB/ALSA audio to one explicitly selected AirPlay speaker,
with automatic silence detection and a terminal menu. Apache-2.0; the bundled
recording has [separate attribution](tests/assets/audio/LICENSE.md).

Supports Raspberry Pi OS 64-bit Bookworm or newer on ARM64. Version 1.0.2 was
physically tested on Debian 13 ARM64 with a Behringer UCA202 and HomePod Stua,
including automatic playback and systemd startup. Other combinations need testing.
Current source version is 1.0.3; the installer selects the latest published stable release.

## Install

Run as your ordinary user:

```sh
bash -c 'installer=$(curl -fsSL https://raw.githubusercontent.com/kyrre-martila/PiAir/main/install.sh) && bash -c "${installer:?PiAir installer download was empty}"'
```

The installer verifies checksums and package contents and asks before installing.
See [installation, updates and rollback](docs/INSTALL.md).

```sh
piair setup       # Input, speaker, authentication, audio settings and boot startup
piair             # Daily menu: status, start/stop, speaker, test audio, settings, logs
```

Setup saves reviewed settings before independently offering startup and an optional
49-second speaker recording test. Test audio does not test USB input. A new speaker
requires explicit approval; existing compatible credentials are reused. See
[setup and recording-test recovery](docs/SETUP.md).

## Daily commands

```sh
piair status
piair start
piair stop
piair restart
piair logs --follow
piair devices
piair audio devices
piair audio monitor
piair audio calibrate
piair config check
piair config show
piair update
piair uninstall
```

Updates and uninstall ask for confirmation and preserve personal configuration and
credentials. Start/stop do not change boot startup. Use the menu to choose speakers
or change settings while stopped. `--help` describes manual configuration, approval
and pairing tools. `audio monitor` and `audio calibrate` are local input tools.

Configuration normally lives in `~/.config/piair/config.toml`, with private
`receiver-state/` beside it. `--config`, `PIAIR_CONFIG` and XDG paths are supported.
See [operation, configuration and troubleshooting](docs/OPERATIONS.md) and
[speaker trust and credentials](docs/SECURITY.md). Buffered live audio is unsupported.

## Building and checking

Install stable Rust with rustfmt/Clippy, a C toolchain, pkg-config and ALSA development
headers (`build-essential pkg-config libasound2-dev` on Debian). Build as your normal user:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
python3 -m unittest discover -s scripts -p 'test_*.py'
```

ARM64 Bookworm packaging and clean-runtime checks run in GitHub Actions on PRs.
Physical audio, recovery and boot startup still require target hardware validation.
