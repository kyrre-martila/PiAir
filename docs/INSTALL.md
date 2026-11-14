# Install PiAir on Raspberry Pi

Use Raspberry Pi OS **64-bit Bookworm or newer**, on a Pi 3, Pi 4, Pi 5,
Pi Zero 2 W, or a compatible ARM64 Raspberry Pi. Check `uname -m`: it must say
`aarch64`. 32-bit Raspberry Pi OS is not supported by these downloads. The
release requires glibc 2.36 or newer and ALSA (`libasound.so.2`). Hardware audio
and AirPlay receiver compatibility still need testing on your devices.

## Install

Open a terminal as your ordinary Pi user. Install the small OS dependencies:

```sh
sudo apt update
sudo apt install curl ca-certificates python3 libasound2
```

Install the current published stable official release:

```sh
curl -fsSL https://raw.githubusercontent.com/kyrre-martila/PiAir/main/install.sh | bash
```

Run this as your ordinary user, **without sudo**. Python 3.9+, glibc 2.36+,
ALSA, sudo and a running systemd manager are required. The installer checks the
system, downloads the latest stable release and matching checksum, and validates
archive members before executing the verified maintenance helper. Read its
system-wide installation prompt and enter `y` or `yes` to approve. Empty input,
EOF and declined prompts never approve installation. Prompts use `/dev/tty`,
including when piped through bash; execution without a controlling terminal fails
closed. Sudo is used only by the existing maintenance helper for system changes.
Nothing automatically starts or enables playback. Then run:

```sh
piair --version
piair setup
```

### Advanced / manual installation

For inspection or troubleshooting, download the entry point before running it:

```sh
curl -fsSL -o install.sh https://raw.githubusercontent.com/kyrre-martila/PiAir/main/install.sh
less install.sh
bash install.sh
```

For a specific version or bootstrap troubleshooting, the manual equivalent is
below. Choose a published stable version deliberately. Run it from a terminal as
your ordinary user; never extract or execute a package before verifying it.

<details>
<summary>Advanced manual download, validation and installation</summary>

```sh
(
  set -eu
  version=1.0.2 # Example only: choose a published stable version deliberately
  package="piair-${version}-aarch64-unknown-linux-gnu"
  download=$(mktemp -d /tmp/piair-install.XXXXXXXXXX)
  trap 'rm -rf -- "$download"' EXIT
  trap 'exit 1' HUP INT TERM
  cd "$download"
  base="https://github.com/kyrre-martila/PiAir/releases/download/v${version}"
  curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 -o "$package.tar.gz" "$base/$package.tar.gz"
  curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 -o SHA256SUMS "$base/SHA256SUMS"
  python3 -I - "$package.tar.gz" <<'CHECKSUM'
import hashlib, pathlib, re, sys
name = sys.argv[1]
lines = [line for line in pathlib.Path("SHA256SUMS").read_text("ascii").splitlines()
         if line.split() and line.split()[-1].lstrip("*") == name]
if (len(lines) != 1 or not re.fullmatch(r"[0-9a-f]{64}  " + re.escape(name), lines[0])
        or hashlib.sha256(pathlib.Path(name).read_bytes()).hexdigest() != lines[0][:64]):
    raise SystemExit("Selected archive checksum missing, ambiguous or mismatched")
CHECKSUM
  python3 -I - "$package" <<'PYTHON'
import pathlib, re, sys, tarfile
package = sys.argv[1]
if not re.fullmatch(r"piair-[0-9]+\.[0-9]+\.[0-9]+-aarch64-unknown-linux-gnu", package):
    raise SystemExit("Invalid PiAir package name")
files = {package + "/" + name for name in (
    "piair", "piair@.service", "maintain.py", "VERSION", "LICENSE", "SHA256SUMS",
    "audio/Pachelbel_Canon_audio_test.wav", "audio/LICENSE.md")}
directories = {package, package + "/audio"}
with tarfile.open(package + ".tar.gz", "r:gz") as archive:
    members, seen = archive.getmembers(), set()
    for member in members:
        regular = member.type in (tarfile.REGTYPE, tarfile.AREGTYPE)
        allowed = (member.name in files and regular and 0 <= member.size <= 48 * 1024 * 1024
                   or member.name in directories and member.isdir())
        if not allowed or member.name in seen:
            raise SystemExit("Unsafe PiAir archive: " + member.name)
        seen.add(member.name)
    if not files <= seen:
        raise SystemExit("Incomplete PiAir archive")
    pathlib.Path(package).mkdir(mode=0o700)
    for member in members:
        if member.name in files:
            destination = pathlib.Path(member.name)
            destination.parent.mkdir(parents=True, exist_ok=True)
            with destination.open("xb") as output:
                output.write(archive.extractfile(member).read())
            destination.chmod(0o755 if destination.name == "piair" else 0o644)
PYTHON
  cd "$package"
  python3 -I maintain.py install "$version" "../$package.tar.gz"
)
```

</details>

The executable is installed in `/usr/local/bin`, normally on the Pi user's PATH.
Setup runs as your ordinary user using the installed executable, without source
code or Cargo. With no existing configuration, it creates
`~/.config/piair/config.toml` with mode 0600; receiver state and credentials use
a private 0700 directory and 0600 files. It guides your audio input and explicit
receiver approval/authentication, and can use the installed 49-second recording.
The service template is installed, but playback and boot startup each require
explicit approval in setup. See [setup](SETUP.md)
and [daily use](DAILY_USE.md).

## Update or uninstall

```sh
piair --version
piair update
piair uninstall
```

Update checks the one stable official release channel only when requested. It
shows the installed and available versions and asks before downloading. It never
downgrades. If your service is running, it asks before stopping it, waits for the
runtime lock, installs, then asks separately whether to start again. Boot startup
is preserved. Declining restart leaves playback stopped. Your settings and pairing
credentials stay in place. GitHub/download/checksum errors leave the installation
unchanged. Normal sudo authorization is required only for system changes.

Uninstall describes the known installed files and asks before removal, stopping
and disabling your service. It preserves `~/.config/piair/config.toml` and the
adjacent `receiver-state/`, including receiver approvals/selection, HAP pairing
credentials, receiver state and all custom settings. Custom `PIAIR_CONFIG` and
`XDG_CONFIG_HOME` locations also remain untouched. Reinstalling can reuse them.
Do not delete private credentials just to troubleshoot an installation.

Check your service (replace USERNAME with your Linux username):

```sh
piair status
systemctl status piair@USERNAME.service
```

## Installation troubleshooting

- **Wrong platform:** use Raspberry Pi OS 64-bit Bookworm or newer. Neither Rust,
  Cargo, Git, Docker nor compilation is needed on the Pi.
- **ALSA missing:** install `libasound2` (or its compatible replacement on newer
  OS releases). Audio device permissions are separate; see [systemd](SYSTEMD.md).
- **No systemd or sudo authorization:** ask the system administrator. Maintenance
  does not modify sudoers or install authorization helpers.
- **Existing unmanaged or changed files:** the installer refuses to overwrite
  them. Ask an administrator to review/back up those files and move only the
  conflicting PiAir files aside before retrying. Do not overwrite a running
  manually installed executable.
- **Another account or foreground process is using PiAir:** stop your own manual
  playback yourself. Shared installations, customized units/drop-ins, other users'
  enabled services during uninstall, and active other-account services require
  deliberate administration. PiAir never kills an unrelated process.
- **Start failed after updating:** the installed version is retained; inspect
  `systemctl status piair@USERNAME.service` and `journalctl -u piair@USERNAME.service -b`.
  Correct the reported cause, then run `piair start`. Nothing enables startup.
- **Interrupted update:** run `piair update` again. It offers to restore the
  previous application before retrying; it never starts playback without asking.
  A durable transaction keeps the old binary, stock template and resources together.
  A failed or interrupted uninstall also restores your service's original boot
  enablement during recovery, without starting playback. If boot-state restoration
  fails, the journal is retained and the error is reported; retry recovery after
  fixing the systemd/authorization problem. Custom boot states need manual review.
  Until recovery completes, the affected service may be masked. If the
  executable/helper is unavailable after an interrupted first install/uninstall,
  an administrator can run the root-owned backup helper:
  `sudo python3 -I /usr/local/share/piair/transaction/maintain.py _commit recover`.
  If no previous helper exists (first install), repeat the official installation
  command; it performs recovery first, then repeat once to install.
  The maintenance mask persists across reboot until recovery finishes, preventing
  boot startup from using a partially replaced installation. Never delete the transaction directory
  or manually replace individual files to bypass recovery.

To report an error, include the exact maintenance error, `piair --version`,
`uname -m`, `/etc/os-release`, and relevant service status. Review output before
posting; never include config/credential contents or pairing codes.

## Installed files and integrity

The archive has one `piair-VERSION-aarch64-unknown-linux-gnu/` directory containing
`piair`, `piair@.service`, `maintain.py`, `VERSION`, `LICENSE` (Apache-2.0), `SHA256SUMS`, and
`audio/{Pachelbel_Canon_audio_test.wav,LICENSE.md}`. The WAV is external to the
executable; its unchanged attribution preserves the recording's separate
**CC BY-SA 3.0** license. PiAir code remains Apache-2.0.

Root owns `/usr/local/bin/piair` (0755), `/etc/systemd/system/piair@.service`
(0644), and `/usr/local/share/piair/` resources and maintenance metadata (0644).
No file capabilities are installed. The unchanged stock service supplies
CAP_NET_BIND_SERVICE to the normal user with NoNewPrivileges and existing
restrictions; the standalone development capability helper stays separate.

The entry point trusts the official repository's `main/install.sh` over HTTPS.
It uses GitHub's `/releases/latest` endpoint and additionally rejects draft,
prerelease and noncanonical version tags; it constructs asset URLs itself from
that stable version, never from arbitrary API asset URLs. It verifies exactly one
matching archive checksum before extracting an allowlist of regular files.
Unexpected, missing, duplicate, oversized, linked or special members are refused.
Temporary files live in a private directory and are cleaned on success, failure,
HUP, INT and TERM (SIGKILL/power loss cannot run cleanup).

Downloads use HTTPS and SHA-256 checksums. Checksums detect corruption or a
mismatched artifact; they are **not independent proof of authenticity** if the
GitHub release source itself is compromised. Updates execute the installed
maintenance helper, not a downloaded installation script. The new executable's
`--version` is checked as the normal user before privileged replacement.

Maintenance verifies known installed hashes before changes and refuses customized
files. Per-file replacements use synced temporary files and rename; a root-owned,
synced backup journal permits rollback across the complete file set. Installation
is serialized, the service is explicitly stopped and temporarily masked against
restarts, and existing per-user kernel locks are held during replacement. Recovery
must complete before another transaction. Configuration is never migrated, copied,
rewritten, or removed. Do not run concurrent administrative service/file changes
while maintenance is in progress.

## Maintainer release validation

Push an explicitly authorized `vMAJOR.MINOR.PATCH` tag matching `Cargo.toml` to
publish later; this change creates no tag. The release workflow builds GNU ARM64
in Debian Bookworm on the native ARM64 runner. It checks ELF architecture, ELF
interpreter, dynamic dependencies, maximum required glibc symbol (<= 2.36), `ldd`
and startup, and then starts the executable with only runtime dependencies in a
fresh Bookworm container. PRs build/test packages but cannot publish releases.
Assets are uploaded to a draft and only published after all uploads succeed.
A failed publication can leave a draft; inspect/remove that draft deliberately
before retrying. There is no external release service or personal access token.
