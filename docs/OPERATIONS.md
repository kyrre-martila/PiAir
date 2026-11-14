# Operation and troubleshooting

Run `piair` in a terminal for status, start/stop, speaker selection, a 49-second
recording test, settings and logs. The recording test passes only when you confirm
hearing it; it does not exercise the USB input. Setup and the menu ask before
stopping playback and before restoring it. See [setup](SETUP.md).

```sh
piair status
piair start
piair stop
piair restart
piair logs --follow
piair devices
piair audio devices
piair audio monitor --duration-secs 30
piair audio calibrate --duration-secs 30
piair config check
piair config show
```

Monitor and calibrate open only the local input. Calibrate with the source stopped;
it proposes thresholds and saves only after confirmation. The legacy `[audio]`
monitor/calibration detector uses separate start/stop thresholds; `[playback]`
automatic playback uses one threshold and qualification durations. They are not
interchangeable. Reconfigure with `piair setup` while stopped, then start explicitly.

## Configuration

Default configuration is `~/.config/piair/config.toml`. Precedence is `--config`,
`PIAIR_CONFIG`, `XDG_CONFIG_HOME/piair/config.toml`, then the HOME default. Private
`receiver-state/` lives beside the resolved configuration. Keep service and terminal
invocations on the same configuration. See `config.example.toml` for the schema.

`[playback]` requires `capture_device`, canonical `receiver_id` and `authentication`
(`stored-hap` or `own-tone-transient`). Defaults: threshold -45 dBFS, start 1000 ms,
stop 30000 ms. Allowed ranges are -120..=0 dBFS, 20..=9000 ms start and
20..=120000 ms stop. Optional volume is -60..=-10 dB and may persist on the speaker;
omitting it preserves volume unless saved. `config set-playback --preserve-volume`
clears a saved volume. Other changes can be scripted with `config set-playback --help`.
Configuration never grants speaker approval or changes credentials.

## Service and timing permissions

The installer supplies `piair@USERNAME.service`, running as your ordinary account.
Setup separately offers boot startup and the recording test. Start/stop commands
do not change boot startup. `piair status` reporting Ready means input handling is
prepared, not that audio is audible. Source builds can inspect the installed
service but cannot control it; use the trusted installed executable.

```sh
systemctl status piair@USERNAME.service
journalctl -u piair@USERNAME.service -b
```

The supported service entry point is `piair run --allow-unattended`. Its explicit
opt-in is required each invocation; it does not approve, select, pair or fall back
between authentication methods. It handles activity, silence, bounded reconnect
and Ctrl+C/SIGTERM shutdown. The hidden `service-recording --saved HASH` worker is
only for the installed temporary recording service; it checks the saved config,
installed executable and service policy before playback. These are the only
non-user-facing commands.

### Timing permissions

Working HomePod playback uses local UDP 319/320. The stock systemd service and
temporary recording worker receive CAP_NET_BIND_SERVICE with NoNewPrivileges;
the interactive wizard needs no file capability. A local bind permission error is
not a pairing error. Use the installed service; do not run PiAir as root, apply
setcap to the managed executable, weaken service permissions or change port sysctls.
An address-in-use error means another process owns the timing ports.

For custom config paths, use a systemd drop-in setting an absolute `PIAIR_CONFIG`
or `XDG_CONFIG_HOME`. Services do not read shell startup files. After unit edits,
run `sudo systemctl daemon-reload`, then restart explicitly. Keep the account's
ALSA and journal access permissions appropriate; avoid a root service.

## Common problems

- No speaker: check the same local network, multicast/mDNS and speaker power.
  `devices` and `receivers` show unverified advertisements and approval state.
- No USB audio: use `audio devices` and `audio monitor`; prefer a `plughw:` input
  for ALSA conversion. Live client audio is stereo S16 at 44100 Hz. A driver that
  silently stops producing data may produce silence until it reports an error.
- Starts/stops at the wrong level: measure the idle source, review threshold and
  qualification times, then test quiet music. Capture queues are bounded and stale
  input is discarded; leading-edge retention and latency depend on hardware.
- Authentication or changed receiver identity: review [security](SECURITY.md).
  Never switch methods or delete credentials just because playback is silent.
- Service unavailable or test recording missing: reinstall the official package;
  it must contain both WAV and attribution. See [recovery](INSTALL.md).
- Logs denied: check journal access. `-v` adds readable debug, `-vv` adds trace;
  unattended runtime logs remain structured JSON. Review logs before sharing.

Version 1.0.2 was physically tested with UCA202 and HomePod Stua on Debian 13 ARM64.
Other device combinations need testing. Hardware checks after changes: sustained
USB/HomePod audio, silence start/stop, input unplug recovery, network reconnect,
cancellation and boot startup. Buffered live audio is unsupported.
