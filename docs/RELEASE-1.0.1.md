# PiAir 1.0.1 installation fix

Version 1.0.0's first clean installation on Raspberry Pi OS 64-bit Debian 13
Trixie stopped after confirmation, before installing the executable. Its strict
command wrapper rejected `systemctl list-unit-files piair@*.service` because
systemd 257 returns status 1 with empty stdout and stderr when no unit files
match. This is a legitimate empty query result, not evidence of a broken manager.

The installer now queries the complete unit-file inventory and selects PiAir
units locally. A normal Debian system has other service units, so the inventory
succeeds even before the PiAir template exists. Nonzero inventory results and query diagnostics on stderr still
fail; connection and permission errors are never converted to empty results.
Inventory rows and service properties are validated before safety decisions.

`systemctl show --all` explicitly includes empty properties (which systemctl
otherwise omits). The two array properties `EnvironmentFiles` and `ExecStart`
are normalized to empty when absent: their custom systemctl printers emit no
line for an empty array, even with `--all`. All scalar properties remain required.
The command is also allowed to return status 1 only for a complete,
consistent `LoadState=not-found`, inactive, PID 0 record with all service-specific
properties empty and no diagnostic on stderr. Loaded units returning nonzero,
missing or duplicate properties, and unexpected load states fail safely.

The complete installation test also caught systemd reporting the temporary mask
fragment as the instance symlink path, rather than `/dev/null`. Recovery now
accepts either representation only for a masked unit whose actual instance
symlink points to `/dev/null`. Custom units and other symlinks remain refused.
Destination ownership and permission checks now run before preparing a fresh
transaction, so an unsafe parent fails before masking or writing application files.

The subprocess regression in `scripts/test_maintenance_systemctl.py` models
Debian 13's status 1 for a filtered empty inventory and the missing-template
`show` response. It runs the real Services adapter through fresh installation,
checks every managed destination and manifest, and asserts no start or enable
command occurs. It also covers stock and active services, another account,
customizations, malformed output, actual execution errors, preflight retry and
fresh-install rollback with unchanged personal data. An actual `systemctl
--root` test exercises the installed systemd unit-file implementation against an
isolated inventory. A read-only integration test exercises missing-template
inspection against a running system manager on the systemd CI runner. An
explicitly opted-in disposable-runner test in `scripts/test_first_install_systemd.py`
also performs a real system-wide installation with an inert ELF fixture, injects
a post-replacement failure, verifies rollback and retry, then checks the installed
template is loaded, inactive and disabled. All systemctl operations and filesystem
transactions in that test are real; only hardware prerequisites and the remote
checksum lookup are substituted.

The existing Rust checks, Python/systemd suite, native ARM64 Bookworm packaging,
ELF/glibc checks, checksums and clean Bookworm runtime checks remain required.
The ARM64 packaging job now also runs the new regression suite as a normal user.
No AirPlay, playback, authentication or configuration behavior changes.

This prepares v1.0.1 only. The existing public v1.0.0 release remains unchanged;
creating a version tag and publishing a release are separate maintainer actions.
