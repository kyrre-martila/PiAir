# Running PiAir with systemd

The optional `systemd/piair@.service` is a **system** service template for Raspberry
Pi OS 64-bit (Bookworm/systemd 252 or later). An instance names an existing Linux
account. PiAir runs as that account, in the foreground, without a terminal. It
survives SSH logout; optional boot enablement is an explicit operator action.
Installing files neither starts nor enables playback. There is no installer,
updater or setup wizard in this change.

## Prepare as the PiAir user (no sudo)

First use the existing interactive CLI to explicitly approve, select and, when
required, pair your receiver. Save and test `[playback]` using
[the runtime workflow](RUNTIME.md#persistent-playback-configuration):

```sh
piair config check
piair config show
piair run --allow-unattended
# Stop the foreground test with Ctrl+C before starting the service.
```

The service executes exactly `/usr/local/bin/piair run --allow-unattended`.
It never approves, selects, pairs, prompts, or switches receivers. Authentication
and discovery retain all runtime checks. Existing HAP credentials are reused;
there is no new configuration or credential model.

`User=%i` resolves the account through NSS, including its supplementary groups.
`WorkingDirectory=~` uses that account's actual home, including homes outside
`/home`. Systemd supplies HOME for `User=` services; do not use `%h` in this
system template because it refers to the system manager's home. A pre-start
check rejects effective UID 0, including aliases of root. Choose an existing
non-root account that owns the configuration and private receiver state.

System services do not read shell startup files or inherit your SSH environment.
Normal application precedence remains `--config`, then `PIAIR_CONFIG`, then
`XDG_CONFIG_HOME/piair/config.toml`, then `HOME/.config/piair/config.toml`.
The template supplies no config flag or environment override. If you normally
use PIAIR_CONFIG or XDG_CONFIG_HOME, configure the same **absolute** path using
`sudo systemctl edit piair@USERNAME.service` (replace USERNAME throughout):

```ini
[Service]
Environment="PIAIR_CONFIG=/absolute/existing/path/config.toml"
# Alternatively: Environment="XDG_CONFIG_HOME=/absolute/existing/config-root"
```

Receiver state is `receiver-state/` beside the resolved configuration, including
persistent selection, HAP credentials and the lazily created client identity.
Do not move or copy credentials, repeat pairing, or automatically change their
ownership/permissions. Existing checks require a user-owned nonsymlink 0700 state
directory and owned regular 0600 files. Confirm the service uses the same config
as the successful foreground invocation. Relative custom paths resolve from the
account's home; absolute paths avoid ambiguity.

## Manual installation for testing (sudo commands)

Build as the ordinary user with `cargo build --release --locked`. Review the
unit, then install the trusted binary without file capabilities. Keep the binary
and unit root-owned and unwritable by the service account:

```sh
sudo install -o root -g root -m 0755 target/release/piair /usr/local/bin/piair
sudo install -o root -g root -m 0644 systemd/piair@.service /etc/systemd/system/piair@.service
sudo systemd-analyze verify /etc/systemd/system/piair@.service
sudo systemctl daemon-reload
sudo systemctl start piair@USERNAME.service
```

No `setcap` is needed for this binary. For upgrades, stop the service, replace the
binary using the same install command, and start it explicitly. Systemd grants
capabilities on each start, so replacing the binary requires no capability
reapplication. Do not install a set-ID or file-capability-bearing binary here:
privileged exec transitions can clear ambient capabilities.

Manage the service with ordinary commands (sudo required for mutations):

```sh
sudo systemctl stop piair@USERNAME.service
sudo systemctl restart piair@USERNAME.service
sudo systemctl enable piair@USERNAME.service
sudo systemctl disable piair@USERNAME.service
systemctl status piair@USERNAME.service
journalctl -u piair@USERNAME.service -b
journalctl -u piair@USERNAME.service -f
```

Enable affects subsequent boots; disable does not stop a running service. Restart
after changing playback configuration. After editing a unit/drop-in, run
`sudo systemctl daemon-reload` before restarting. Journal access may require sudo
or membership in the host's journal-reading group. Logs include JSON stderr
runtime events and stdout diagnostics; do not publish private logs unreviewed.

## Privileges and supervision

The system manager grants only CAP_NET_BIND_SERVICE through both
AmbientCapabilities and CapabilityBoundingSet. NoNewPrivileges prevents exec
from acquiring extra privileges; it does not remove the capability already
granted by systemd. This combination allows non-root PTP binds on UDP 319/320
without executable xattrs. The capability permits binding any privileged TCP/UDP
port, not only PTP; it grants no network administration or raw sockets.
See [standalone CLI permissions](PTP_PORT_PERMISSIONS.md) for the separate helper.

ProtectSystem=full makes `/usr`, `/boot` and `/etc` read-only. ProtectHome=no
preserves existing private home access and atomic state writes. Ordinary DAC
permissions still apply. Custom config/state under a protected directory needs
an explicitly reviewed `ReadWritePaths=/absolute/config-directory` drop-in;
the directory must already exist and remain securely owned. PrivateTmp isolates
`/tmp` and `/var/tmp`: if existing config/state or an ALSA plugin requires shared
files there, explicitly set `PrivateTmp=no` or use durable configuration storage.
No paths or permissions are changed automatically.

Kernel tunables/modules/control groups and set-ID creation are protected.
PrivateDevices=no preserves `/dev/snd`; no device policy hides ALSA. PrivateNetwork=no
preserves mDNS multicast, TCP authentication, UDP RTP/feedback/PTP and the existing
AF_UNIX abstract process lock. Allowed address families include Unix, IPv4, IPv6
and netlink (interface enumeration); no raw-socket capability is granted.
The lock remains per effective Linux user in the host network namespace, not
global across accounts. A competing automatic runtime fails clearly and cannot
stop or signal its owner. Stop a manual runtime yourself before starting service;
do not remove lock files or add a separate lock.

Wants/After network-online.target orders startup after the host's configured
online wait; it does **not** guarantee a receiver is reachable. There is no hard
network or sound-device dependency that would terminate PiAir on device/service
changes. Receiver discovery, connection retries and existing fresh-audio backoff
remain application responsibilities. No occupancy detection or switching is added.

Restart=on-failure waits 10 seconds between failed invocations. At most three
starts are permitted per 300 seconds, including manual starts. Persistent config,
permission or lock errors therefore settle into a failed/rate-limited state.
After fixing the cause, use `sudo systemctl reset-failed piair@USERNAME.service`
and start again. Systemd does not restart an operator-requested stop, even if
teardown reports failure. Successful exits are not automatically restarted.

Stop sends SIGTERM directly to PiAir's control group. Existing cancellation stops
monitoring/media, attempts bounded TEARDOWN, joins capture/timing/media workers,
closes ALSA and sockets, and releases the process lock. Cleanup failures remain
nonzero and visible in the journal. TimeoutStopSec=30s bounds a stuck stop; after
that systemd may SIGKILL, which cannot guarantee remote TEARDOWN. Blocking external
ALSA plugins retain the limitations documented in [RUNTIME.md](RUNTIME.md).
There is no daemon fork, PID file, notification protocol or watchdog.

## Diagnosis

- **PTP permission denied:** inspect `systemctl cat piair@USERNAME.service` and
  `systemctl show piair@USERNAME.service -p User -p AmbientCapabilities -p CapabilityBoundingSet -p NoNewPrivileges`.
  Check for overriding drop-ins and file capabilities with `getcap /usr/local/bin/piair`.
  While running, inspect `CapEff`, `CapBnd`, `CapAmb` and `NoNewPrivs` in
  `/proc/MAINPID/status` (MAINPID from `systemctl show -p MainPID --value ...`).
  CAP_NET_BIND_SERVICE is bit 10 (`0x400`). A container or host security policy
  can constrain grants. Address-in-use errors indicate another port owner, not
  missing capabilities; do not broaden privileges or lower global port sysctls.
- **ALSA denied/missing:** inspect `id USERNAME`, `ls -l /dev/snd` and the journal.
  SSH seat ACLs may not survive logout. If the host uses group `audio` for capture,
  explicitly grant the chosen user membership with `sudo usermod -aG audio USERNAME`,
  then restart the service to refresh groups. This grants audio-device access;
  assess it deliberately. Test `piair audio devices` as that user and check the
  saved capture device and competing users of the device. Do not use root playback.
- **Missing config/selection/credentials:** compare the foreground `piair status`
  config path with the service environment/drop-in. Check the account home via
  `getent passwd USERNAME`, path traversal permissions, private state modes and
  writable parent directories. No login-shell variables are imported. Absent
  default config leads to missing playback settings, never automatic enrollment.
  Fix the path or explicit setup as the owning user; do not relax credential checks.

## Revert (sudo commands)

```sh
sudo systemctl disable --now piair@USERNAME.service
# Repeat for any other enabled/running instances before removing the template.
sudo rm /etc/systemd/system/piair@.service
sudo systemctl daemon-reload
sudo systemctl reset-failed piair@USERNAME.service
```

Remove only drop-ins you created after reviewing their paths. Keep configuration
and receiver-state intact for the CLI. Optionally remove `/usr/local/bin/piair`
if no CLI uses it. A foreground binary without file capabilities may need the
standalone capability helper for PTP; the service grant ends with the process.

## Automated checks and remaining hardware validation

Run `python3 -m unittest discover -s scripts -p 'test_*.py'`. Static tests assert
the complete service contract and use systemd-analyze verify when installed.
Verification uses user scope with a temporary runtime directory and substitutes
only `/usr/bin/true` for the uninstalled executable; no service is started.
This checks parsing, not system-manager privilege enforcement. On the Pi, also
verify the installed system unit with the sudo command above. CI retains Rust
fmt/clippy/tests/release checks and the native ARM64 release build. Existing
process tests exercise saved playback, authorization revocation, per-user lock
exclusion, SIGTERM and lock release; mock session tests cover bounded cleanup.

GitHub Actions cannot establish real system-service capability propagation,
ALSA group/ACL access after SSH logout, actual multicast/PTP reachability, audible
playback, remote TEARDOWN, boot ordering, reboot startup or host-specific shutdown
timing. On Raspberry Pi OS, explicitly test start/stop/restart, logout, reboot
after enablement, failure restart/rate limits, concurrent manual execution,
credential revocation and resource release after stopping during active playback.
Record the hardware/OS and journal outcomes; this PR claims no new hardware result.
