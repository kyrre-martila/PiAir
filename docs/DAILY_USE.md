# Daily PiAir use

Run `piair` over SSH with a TTY for numbered choices. Status is read-only and
never takes the playback lock, opens audio, scans receivers or connects to them.
The menu refreshes saved settings and service state after each action.
Use `piair --help` for direct and advanced commands.

- **Configuration incomplete:** choose Run piair setup. Setup is also available
  under Settings for later changes. Explicitly named missing config files and
  invalid TOML fail with their path; correct the file or use the normal default.
- **Service unavailable / not installed:** PiAir can still be configured and run
  in the foreground with explicit timing-port permission (see
  [PTP permissions](PTP_PORT_PERMISSIONS.md)). Use the [release installation procedure](INSTALL.md). Daily commands do not install anything.
- **Authorization denied:** systemctl uses normal system authorization, including
  polkit where configured. Ask the system administrator about service management
  permissions; PiAir does not change sudoers or run playback as root.
- **Custom template, environment or drop-ins:** review and control the service
  manually using [SYSTEMD.md](SYSTEMD.md). Daily controls verify the stock template,
  executable, account, environment and default config path before acting.
- **Another automatic playback instance:** stop your foreground `piair run` or
  automatic capture manually before setup, receiver changes or standalone tests.
  PiAir never kills it or removes a lock file.
- **Running - activity unknown:** systemd reports an active service with a main
  PID. There is no reliable live playback observation. Even a successful local
  playback transmission cannot establish that sound was audible.
- **Receiver authentication not configured:** use `piair setup` for explicit
  enrollment or authentication selection. A different authentication method is
  never tried automatically. Discovery names and addresses are untrusted.
- **Receiver update failed:** inspect the reported persistence result. Selection
  writes are serialized with other selection writers; playback saves reject a
  changed config snapshot. A failed save attempts to restore the old selection. An uncertain config save
  restores the reviewed old config only if the file still matches the attempted
  write; concurrent changes are never overwritten.
  A crash between the two atomic files can leave different receiver IDs. Runtime
  preparation rejects this state; run setup to review and repair before starting.
  Approval explicitly granted before a later failure remains saved.
- **Test recording missing:** install both WAV and LICENSE.md as described in
  [SETUP.md](SETUP.md), or set `PIAIR_AUDIO_DATA_DIR`. Nothing is downloaded.
- **No logs / journal access denied:** `piair logs` reads the latest 40 entries
  for the current account's unit; `piair logs --follow` follows it until Ctrl+C.
  Structured events show their message, errors and runtime event details;
  `piair -v logs` includes additional safe diagnostic fields.
  Empty output can mean no retained records or insufficient journal access.
  Check the journal's permission message and ask the administrator about journal
  access. For full systemd diagnostics use the commands in [SYSTEMD.md](SYSTEMD.md).
  Entries are historical, potentially from earlier service processes; they are
  never used to infer current playback. The service result and MainPID shown by
  status come from systemd's current properties.

Starting or stopping does not enable or disable boot startup. A stopped service
is not automatically started after changing receivers or settings. If the menu
stopped a running service, it offers a separate confirmed start afterward.
No automatic fallback, multiple outputs or receiver occupancy checks are used.

## Cancelled or failed menu actions

Receiver compatibility, existing credentials and saved settings are checked before
stopping playback. The audio test's play/skip choice and recording validation also
happen first. If a receiver change, test or setup fails after a confirmed stop,
the menu reloads the saved settings and checks selection, approval, authentication
and the runtime lock before offering to start PiAir again. It never starts without
your confirmation, changes boot startup, or rolls back an approved receiver as
part of recovery. A service that was already stopped stays stopped.

If settings are inconsistent, credentials are missing, cleanup is uncertain or a
manual runtime owns the lock, recovery explains the problem and leaves playback
stopped. Use `piair status` and `piair setup` to review/fix settings; stop manual
runtimes yourself. Ctrl+C cancels safely without another confirmation. EOF or
cancelling the restart offer also leaves the service stopped with guidance to run
`piair start` later. Logs use readable local timestamps, including the timezone,
when an application timestamp is absent.

## Separate candidate or development executable

Status describes the **installed systemd service**, separately from the invoking
executable. A separate build may inspect status and logs, but cannot start, stop,
restart, enable or activate that service (including setup and menu recovery).
Use the trusted `/usr/local/bin/piair` deliberately to manage its installed unit.
Symlinks to that same executable are supported; a copied or replaced executable
has a different file identity and is refused. Candidate foreground playback stays
available with explicit PTP permission; see [candidate QA](PRE_RELEASE_QA.md).
The friendly-name receiver picker has no technical details menu; use `piair
receivers` or `piair devices` for separate diagnostic output.
