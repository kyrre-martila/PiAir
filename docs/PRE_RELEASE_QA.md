# Unpublished 1.0.2 candidate: Raspberry Pi manual QA

Required physical target: Raspberry Pi OS/Debian 13 Trixie **ARM64**, Behringer
UCA202 and the intended HomePod, with an existing managed PiAir 1.0.1 installation
and saved selection/configuration. Every listening, USB-input, silence and physical
cleanup observation below is **manual and pending**. Automated CI does not supply
these results. Do not publish until they have been recorded for the final commit.

## Preserve the installation and user data

As the ordinary account that owns the current setup:

```sh
uname -m                       # aarch64
cat /etc/os-release             # Trixie
piair --version                 # record installed version (expected 1.0.1)
piair status
systemctl is-active "piair@$(id -un).service"    # record; inactive returns nonzero
systemctl is-enabled "piair@$(id -un).service"   # record; disabled returns nonzero
```

Record the actual config path shown by status; retain any custom `--config`,
PIAIR_CONFIG or XDG configuration settings. Set the following to that actual path:

```sh
PIAIR_QA_CONFIG="$HOME/.config/piair/config.toml"
PIAIR_QA_WORK=$(mktemp -d "$HOME/piair-qa-79.XXXXXXXX")
chmod 700 "$PIAIR_QA_WORK"
cp -a "$(dirname "$PIAIR_QA_CONFIG")" "$PIAIR_QA_WORK/config-backup"
find "$(dirname "$PIAIR_QA_CONFIG")/receiver-state" -type f -name '*.toml' \
  ! -name 'selected.toml' -exec sha256sum {} + > "$PIAIR_QA_WORK/state-before.sha256"
```

Keep backups/hash lists private. Do not post config, codes or credential files.
Record the receiver ID/method and capture device locally using `piair config show`
and `piair receivers`. If active, explicitly stop with `piair stop`; verify inactive
and MainPID=0 using status before testing. Stop any foreground runtime yourself.
Do not enable boot startup. Preserve the original enabled/disabled state.

## Build an isolated candidate

`piair update` reads **official published releases** and cannot install this
unpublished candidate. The current installer also validates against official
release checksums. Do not substitute a candidate into `/usr/local/bin`, change its
manifest, or bypass those checks.

Install the developer prerequisites from [ADVANCED_USAGE.md](ADVANCED_USAGE.md#build-from-source-developers),
including a current Rust toolchain (at least 1.93). Clone/build as your ordinary
user, with no sudo build. PR refs work before merge:

```sh
git clone https://github.com/kyrre-martila/PiAir.git "$PIAIR_QA_WORK/source"
cd "$PIAIR_QA_WORK/source"
git fetch origin refs/pull/79/head
git checkout --detach FETCH_HEAD
git rev-parse HEAD              # compare with the final reviewed PR commit
cargo build --release --locked
PIAIR_QA_BIN="$PWD/target/release/piair"
"$PIAIR_QA_BIN" --version        # piair 1.0.2
piair --version                 # managed installation still 1.0.1
```

Keep the checkout unchanged during testing. For this **separate developer binary**,
review `scripts/grant-ptp-capability.py` and the [permission scope](PTP_PORT_PERMISSIONS.md).
If you explicitly authorize the capability for the candidate, run:

```sh
sudo python3 scripts/grant-ptp-capability.py "$PIAIR_QA_BIN"
getcap "$PIAIR_QA_BIN"           # only cap_net_bind_service=ep
```

This grants privileged-port binding only to that executable; it starts nothing.
It does not alter the installed binary, sudoers, sysctls or pairing credentials.
Repeat verification after any rebuild. Run all PiAir commands below without sudo.

## Exercise the candidate and listen

1. **Fresh/cancelled setup:** create an isolated directory with `mkdir -m 700
   "$PIAIR_QA_WORK/fresh"`, then run `env -u PIAIR_CONFIG
   XDG_CONFIG_HOME="$PIAIR_QA_WORK/fresh" "$PIAIR_QA_BIN" setup`. Verify clean readable prompts,
   UCA202 input grouping/advanced aliases and explicit selection even for one
   input. Cancel before approving a receiver with `q`; repeat using Ctrl+C and
   EOF. No background process should start. This separate config does not use
   or reset your saved credentials.
2. **Service detection:** `piair status` must detect the managed installed unit
   and its real active/boot state. Candidate `"$PIAIR_QA_BIN" --config
   "$PIAIR_QA_CONFIG" status` must safely decline control when its executable
   path differs from the stock unit. This is expected for an isolated build;
   do not edit the stock unit to point at the candidate. Full installed-candidate
   service detection must be rechecked after official supported installation.
3. **Reconfiguration:** run `"$PIAIR_QA_BIN" --config "$PIAIR_QA_CONFIG" setup`.
   Choose the UCA202 input and the intended discovered HomePod. Review Advanced
   details if addresses changed; explicitly reselect when required. Reuse the
   existing saved pairing when applicable. Keep the existing authentication
   method; never switch to transient merely to bypass failed saved credentials.
   Verify one new-approval prompt only when actually unapproved; connection save
   remains separate. Keep detection/volume defaults unless deliberately changing.
4. **Recording:** explicitly choose Play. Physically listen at the HomePod for
   the recording through its full content (up to 49 seconds), correct channels,
   continuity and acceptable level. Record duration and what you actually heard.
   Answer Yes only if audible; packet or protocol success is insufficient. Verify
   volume stays unchanged. Save reviewed settings and decline service activation.
5. **Permission failure:** after a completed candidate test, explicitly revoke
   the candidate's grant with `sudo setcap -r "$PIAIR_QA_BIN"`; verify `getcap`
   has no output. Repeat the setup test if Linux's privileged-port policy requires
   a capability: expect a **local timing-port permission** explanation, no pairing
   blame and no audible-success question. Skip/save or cancel safely. If the host
   already permits unprivileged binds, record this case as not exercised; do not
   change sysctls. Regrant only with the reviewed helper if continuing playback.
6. **Automatic USB audio:** explicitly run `"$PIAIR_QA_BIN" --config
   "$PIAIR_QA_CONFIG" run --allow-unattended`. Play representative music into
   the UCA202; verify sustained input starts audible HomePod playback, including
   quiet openings. Stop the source and verify playback stops after the saved
   silence duration (default 30 s). Start the source again and confirm audible
   recovery. Test several cycles, quiet music, background noise and at least a
   sustained track; record delays, dropouts and any clipped openings.
7. **Cancellation/cleanup:** interrupt a recording and automatic playback with
   Ctrl+C; verify prompt/process exit, no continued audio, and ability to rerun
   without busy ALSA/PTP ports or another-instance failures. Repeat one failure
   such as temporarily disconnecting the USB input or powering down the speaker,
   then reconnect and explicitly rerun. Record errors and cleanup behavior.
   Investigate warnings; do not reinterpret cleanup failure as success. Use `-v`
   or `-vv` only when collecting diagnostics and review before sharing.
8. **Preservation:** run `sha256sum --check "$PIAIR_QA_WORK/state-before.sha256"`
   and inspect any differences locally. Existing credential material must remain
   unchanged; selection/settings change only where explicitly reviewed. Check
   private permissions and compare config with the backup. `piair --version`
   must remain 1.0.1, and installed manifest and boot enablement must be unchanged.
   Stop the candidate before optionally, explicitly starting the existing service
   with `piair start`. Do not restore a backup over concurrent config changes.

Revoke the separate candidate's capability when done (`sudo setcap -r
"$PIAIR_QA_BIN"` if present). Keep the private backup until review is complete.
Record final commit, OS, receiver firmware, auth method, ALSA ID, each observed
result and every failure. Missing audible recording or automatic-cycle verification
blocks release readiness. Native Bookworm package/ELF/glibc/startup CI is separate
from these Trixie hardware observations.
