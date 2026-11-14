# PiAir

PiAir is a lightweight Rust CLI application that sends audio from a Raspberry Pi
ALSA input to one explicitly selected AirPlay speaker. It can start and stop
playback when audio activity changes, with a simple terminal menu for daily use.
Licensed under Apache-2.0; the bundled test recording has separate attribution.

PiAir v1.0.2 has been physically tested on Debian 13 ARM64 with a Behringer
UCA202 and HomePod Stua, including automatic detection, streaming and installed
systemd startup. Compatibility with other devices still requires testing.
See [validation status](docs/DEVELOPMENT.md#validation-status).

## Install and configure

Precompiled packages support **Raspberry Pi OS 64-bit Bookworm or newer** on ARM64
Raspberry Pis (Pi 3/4/5 and Zero 2 W). Follow the
[installation guide](docs/INSTALL.md#install). Install as your ordinary user:

```sh
bash -c 'installer=$(curl -fsSL https://raw.githubusercontent.com/kyrre-martila/PiAir/main/install.sh) && bash -c "${installer:?PiAir installer download was empty}"'
```

The installer selects the current stable official release, verifies its checksum
and archive, and asks before system-wide installation. No Rust, Cargo, Git,
compilation or existing configuration is needed. Then run `piair setup`.

```sh
piair setup       # Save settings, choose boot startup, optionally test speaker audio
piair             # Open the daily menu (also works over SSH with a TTY)
```

Setup guides you through audio input and speaker selection, the connection
method, audio settings, saving, automatic startup and an optional recording test. See [setup and troubleshooting](docs/SETUP.md).
Existing configurations and stored credentials remain usable without pairing again.

Setup saves settings before independently offering boot startup and a 49-second
recording test. The optional test uses systemd permissions as your ordinary user
and preserves speaker volume; it does not test the ALSA input.

## Daily use

Running `piair` in a terminal opens the existing menu:

```text
1. Status
2. Start/Stop PiAir
3. Choose speaker
4. Test audio (49 seconds)
5. Settings
6. Logs

Q. Exit
```

Choice 2 shows **Start PiAir** or **Stop PiAir**, depending on service state.
Choose **Choose speaker** to change the saved speaker. The picker
preserves the configured authentication method and reuses compatible credentials;
a new speaker still needs explicit approval and authentication. If playback is
running, the menu asks to stop it before changing speakers or testing audio.
Incomplete configuration offers the existing setup workflow.

Direct commands are also available:

```sh
piair status
piair start
piair stop
piair restart
piair logs                 # Add --follow to watch until Ctrl+C
piair devices              # Discover AirPlay advertisements
piair audio devices        # List ALSA capture inputs
piair config show
piair config check
```

Start/stop/restart control the installed current-account service as a non-root
user through normal system authorization. Start/stop do not change boot settings.
Separate development builds can inspect the installed service but cannot control
it; use the installed executable deliberately. Receiver technical details are
available through `piair receivers` or `piair devices`, outside the friendly-name
picker. Status shows **Service: Running / Playback: Ready** only for a verified
installed service with a live main process and valid saved settings. Ready means
PiAir is prepared to handle input; it does not mean audio is streaming. A stopped
service shows **Playback: Not running**. Use `piair -v status` for service details.
The audio test passes only when you confirm hearing the recording.
See [daily use and troubleshooting](docs/DAILY_USE.md) and [systemd](docs/SYSTEMD.md).

## Update or uninstall

```sh
piair update
piair uninstall
```

Both require explicit confirmation. Updates preserve settings and credentials;
uninstall preserves personal configuration and credentials by default. See
[installation, rollback and recovery](docs/INSTALL.md).

## Configuration and troubleshooting

Settings normally live in `~/.config/piair/config.toml`, with private selection
and credentials in adjacent `receiver-state/`. `--config PATH`, `PIAIR_CONFIG`
and XDG configuration paths remain supported. Use the menu's **Settings** or
`piair setup` to reconfigure while stopped, then explicitly start when ready. Manual configuration is
available through `piair config --help` and [runtime settings](docs/RUNTIME.md).
The legacy `[audio]` monitor/calibration thresholds and `[playback]` activity
settings retain their different detector semantics; no migration is required.

- [Installation and update troubleshooting](docs/INSTALL.md#installation-troubleshooting)
- [Setup, authentication and the 49-second audio test](docs/SETUP.md)
- [Daily controls, receiver changes and logs](docs/DAILY_USE.md)
- [Audio monitoring, calibration and manual receiver configuration](docs/ADVANCED_USAGE.md)
- [PTP permissions for standalone diagnostics](docs/PTP_PORT_PERMISSIONS.md)
- [Developer guide, retained diagnostics and validation evidence](docs/DEVELOPMENT.md)

`piair audio monitor` and `piair audio calibrate` remain available for input
troubleshooting. The systemd runtime `piair run --allow-unattended` remains
supported; see [unattended operation](docs/RUNTIME.md). Specialized diagnostics
are hidden from ordinary help but retain their arguments, aliases and safety
checks. Buffered live audio remains explicitly blocked pending clock-adoption
evidence; see [buffered lifecycle](docs/BUFFERED_LIFECYCLE.md).
