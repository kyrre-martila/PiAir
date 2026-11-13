#!/usr/bin/env python3
"""PiAir's installed, deliberately small stable-channel maintenance mechanism.

Only the private _commit entry needs root. No configuration paths are touched.
Tests inject a temporary root and service adapter directly, never through CLI/env.
"""
import contextlib
import ctypes
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import pwd
import re
import shutil
import socket
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

REPO = "https://github.com/kyrre-martila/PiAir"
API = "https://api.github.com/repos/kyrre-martila/PiAir/releases/latest"
BASE = Path("/usr/local/share/piair")
HELPER = BASE / "maintain.py"
VERSION = BASE / "VERSION"
MANIFEST = BASE / "installation.json"
JOURNAL = BASE / "transaction"
COMPLETED = BASE / "completed-transaction"
WAV = "Pachelbel_Canon_audio_test.wav"
# Archive-relative name -> installed absolute name, mode.
FILES = {
    "piair": ("/usr/local/bin/piair", 0o755),
    "piair@.service": ("/etc/systemd/system/piair@.service", 0o644),
    "maintain.py": (str(HELPER), 0o644),
    "VERSION": (str(VERSION), 0o644),
    "LICENSE": (str(BASE / "LICENSE"), 0o644),
    "audio/" + WAV: (str(BASE / "audio" / WAV), 0o644),
    "audio/LICENSE.md": (str(BASE / "audio/LICENSE.md"), 0o644),
}


def require(ok, message):
    if not ok:
        raise RuntimeError(message)


def version(value):
    require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", value) is not None,
            "Expected a stable semantic version (for example 1.0.0)")
    parts = value.split(".")
    require(all(p == str(int(p)) for p in parts), "Noncanonical version")
    return tuple(map(int, parts))


def filename(v):
    version(v)
    return f"piair-{v}-aarch64-unknown-linux-gnu.tar.gz"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def fetch(url, limit=64 * 1024 * 1024):
    require(url.startswith("https://"), "HTTPS required")
    request = urllib.request.Request(url, headers={"User-Agent": "PiAir-maintenance"})
    # urllib follows redirects; reject any downgrade to cleartext.
    class HTTPSOnly(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, req, fp, code, msg, headers, newurl):
            require(newurl.startswith("https://"), "Non-HTTPS redirect refused")
            return super().redirect_request(req, fp, code, msg, headers, newurl)
    with urllib.request.build_opener(HTTPSOnly()).open(request, timeout=30) as response:
        data = response.read(limit + 1)
    require(len(data) <= limit, "Release response exceeds size limit")
    return data


def checksum(v):
    name = filename(v)
    text = fetch(f"{REPO}/releases/download/v{v}/SHA256SUMS", 4096).decode("ascii")
    matches = [line.split()[0] for line in text.splitlines()
               if re.fullmatch(r"[0-9a-f]{64}  " + re.escape(name), line)]
    require(len(matches) == 1, "Release checksum missing or ambiguous")
    return matches[0]


def unpack(data, v, expected):
    require(digest(data) == expected, "Release checksum mismatch; nothing installed")
    import io
    version(v)
    prefix = f"piair-{v}-aarch64-unknown-linux-gnu/"
    result, seen = {}, set()
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for item in archive:
            require(item.name not in seen, "Duplicate archive entry: " + item.name)
            seen.add(item.name)
            if item.isdir() and item.name == prefix.rstrip("/"):
                continue
            require(item.name.startswith(prefix), "Unexpected archive path")
            name = item.name[len(prefix):]
            if item.isdir() and name in ("", "audio"):
                continue
            require(name in FILES or name == "SHA256SUMS", "Unexpected release file: " + name)
            require(item.type in (tarfile.REGTYPE, tarfile.AREGTYPE) and 0 <= item.size <= 48 * 1024 * 1024,
                    "Invalid, duplicate or oversized release file: " + name)
            result[name] = archive.extractfile(item).read()
    require(set(result) == set(FILES) | {"SHA256SUMS"}, "Missing release files")
    require(result["VERSION"] == (v + "\n").encode(), "Release version mismatch")
    checks = result.pop("SHA256SUMS").decode("ascii").splitlines()
    require(sorted(checks) == sorted(digest(value) + "  " + key for key, value in result.items()),
            "Package file checksum mismatch")
    elf(result["piair"])
    return result


def elf(data):
    require(len(data) >= 64 and data[:6] == b"\x7fELF\x02\x01" and
            struct.unpack_from("<H", data, 18)[0] == 183, "Executable must be ARM64 Linux ELF")
    symbols = re.findall(rb"GLIBC_([0-9]+)\.([0-9]+)(?:\.([0-9]+))?", data)
    require(symbols and max(tuple(int(x or b"0") for x in s) for s in symbols) <= (2, 36, 0),
            "Executable requires glibc newer than Bookworm 2.36")


def prerequisites():
    require(platform.system() == "Linux" and platform.machine() == "aarch64",
            "PiAir releases require Linux ARM64 (uname -m: aarch64), Raspberry Pi OS 64-bit")
    libc, release = platform.libc_ver()
    require(libc == "glibc" and tuple(map(int, release.split(".")[:2])) >= (2, 36),
            "Raspberry Pi OS Bookworm or newer, glibc >= 2.36 required")
    try:
        ctypes.CDLL("libasound.so.2")
    except OSError as error:
        raise RuntimeError("ALSA runtime missing: sudo apt install libasound2") from error
    require(shutil.which("systemctl") and Path("/run/systemd/system").is_dir(),
            "A running systemd system manager is required for this installation")


def confirm(message):
    require(sys.stdin.isatty() and sys.stdout.isatty(), "An interactive terminal is required")
    return input(message + " [y/N] ").strip().lower() in ("y", "yes")


def run(*args):
    p = subprocess.run(args, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    require(p.returncode == 0, "Command failed: " + " ".join(args) + "\n" + p.stderr.strip())
    return p.stdout.strip()


class Services:
    def ctl(self, *args):
        return run("/usr/bin/systemctl", "--system", "--no-pager", *args)

    def properties(self, unit):
        fields = "LoadState,ActiveState,User,FragmentPath,DropInPaths,Environment,EnvironmentFiles,MainPID,UnitFileState,ExecStart,WorkingDirectory"
        args = ("/usr/bin/systemctl", "--system", "--no-pager", "show", unit, "--all", "--property=" + fields)
        result = subprocess.run(args, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
        properties = {}
        for line in result.stdout.splitlines():
            key, sep, value = line.partition("=")
            require(sep and key in fields.split(",") and key not in properties,
                    "Malformed systemctl show output: " + unit)
            properties[key] = value
        # systemctl's custom array printers omit empty arrays even with --all.
        # Their absence means no entries; scalar properties must still be present.
        for key in ("EnvironmentFiles", "ExecStart"):
            properties.setdefault(key, "")
        require(set(properties) == set(fields.split(",")),
                "Incomplete systemctl show output: " + unit + "; missing " +
                ",".join(sorted(set(fields.split(",")) - set(properties))))
        # Some systemd versions return EXIT_FAILURE for a missing unit even with
        # explicit properties. Accept only the complete, inert not-found record.
        absent = (properties["LoadState"] == "not-found" and
                  properties["ActiveState"] == "inactive" and properties["MainPID"] == "0" and
                  all(not properties[key] for key in fields.split(",")
                      if key not in ("LoadState", "ActiveState", "MainPID")))
        require(not result.stderr.strip() and (result.returncode == 0 or
                (result.returncode == 1 and absent)),
                "Command failed: " + " ".join(args) + "\n" + result.stderr.strip())
        require(properties["LoadState"] in ("loaded", "masked") or absent,
                "Unexpected systemd load state: " + unit)
        require(properties["ActiveState"] in ("active", "reloading", "inactive", "failed",
                "activating", "deactivating", "maintenance", "refreshing"),
                "Unexpected systemd active state: " + unit)
        require(properties["MainPID"].isdigit(), "Invalid systemd MainPID: " + unit)
        return properties

    def listed_units(self):
        units = set()
        for command in ("list-units", "list-unit-files"):
            # systemd 257 returns 1 for a filtered unit-file inventory with zero
            # matches. Query the complete inventory instead: errors remain errors.
            pattern = ("piair@*.service",) if command == "list-units" else ()
            output = self.ctl(command, *pattern, "--no-legend", "--plain")
            for line in output.splitlines():
                columns = line.split()
                require(len(columns) >= (4 if command == "list-units" else 2),
                        "Malformed systemctl inventory")
                if command == "list-unit-files":
                    require(len(columns) in (2, 3) and columns[1] in (
                        "enabled", "enabled-runtime", "linked", "linked-runtime", "alias",
                        "masked", "masked-runtime", "static", "disabled", "indirect",
                        "generated", "transient", "bad", "not-found") and
                        (len(columns) == 2 or columns[2] in ("enabled", "disabled", "-")),
                        "Malformed systemctl unit-file state")
                else:
                    require(columns[1] in ("loaded", "not-found", "bad-setting", "error", "merged", "masked") and
                            columns[2] in ("active", "reloading", "inactive", "failed", "activating",
                                           "deactivating", "maintenance", "refreshing"),
                            "Malformed systemctl unit state")
                name = columns[0]
                require(re.fullmatch(r"[^\s/]+\.[a-z]+", name), "Malformed systemctl unit name")
                if name.startswith("piair@") and name.endswith(".service"):
                    require(re.fullmatch(r"piair@[a-zA-Z0-9_-]*\.service", name),
                            "Unexpected PiAir service instance; deliberate administration required")
                    units.add(name)
        return units

    def inspect(self, uid, removing=False, recovery=False):
        user = pwd.getpwuid(uid).pw_name
        require(uid != 0 and re.fullmatch(r"[a-zA-Z0-9_][a-zA-Z0-9_-]*", user),
                "Run maintenance as the intended normal Linux user")
        own = "piair@" + user + ".service"
        units = self.listed_units() | {own}
        active = False
        for unit in sorted(units - {"piair@.service"}):
            p = self.properties(unit)
            state = p.get("ActiveState")
            busy = state not in ("inactive", "failed")
            enabled = p.get("UnitFileState") in ("enabled", "enabled-runtime", "linked", "linked-runtime")
            require(unit == own or not (busy or (removing and enabled)),
                    f"Another account uses {unit}; ask an administrator to stop/review it deliberately")
            if unit == own:
                active = busy
                require((p.get("FragmentPath") in ("", FILES["piair@.service"][0]) or
                         (recovery and p.get("FragmentPath") == "/dev/null" and
                          os.path.islink("/etc/systemd/system/" + own) and
                          os.readlink("/etc/systemd/system/" + own) == "/dev/null")) and
                        not p.get("DropInPaths") and not p.get("Environment") and
                        not p.get("EnvironmentFiles") and p.get("User", "") in ("", user),
                        "Customized PiAir service; deliberate administration required")
                if p.get("FragmentPath") == FILES["piair@.service"][0]:
                    start = p.get("ExecStart", "")
                    require(start.startswith("{ path=/usr/local/bin/piair ; argv[]=/usr/local/bin/piair run --allow-unattended ;")
                            and start.count("argv[]=") == 1 and p.get("WorkingDirectory") == "~",
                            "Unexpected loaded service executable or working directory; review systemd configuration")
        # No other foreground/legacy process using the shared executable may be replaced.
        service_pid = self.properties(own).get("MainPID", "0")
        for proc in Path("/proc").iterdir():
            if proc.name.isdigit():
                try:
                    target = os.readlink(proc / "exe").removesuffix(" (deleted)")
                except (FileNotFoundError, PermissionError, ProcessLookupError):
                    continue
                require(target != FILES["piair"][0] or proc.name == service_pid,
                        "A foreground or unrelated PiAir process is running; stop it yourself first")
        return own, active

    def quiesce(self, unit):
        # A synchronous operator stop cancels Restart=on-failure, including restart delay.
        self.ctl("stop", unit)
        require(self.properties(unit).get("ActiveState") in ("inactive", "failed"),
                "Service did not stop")

    def guard(self, unit):
        mask = Path("/etc/systemd/system") / unit
        require(not mask.exists() and not mask.is_symlink(), "Existing instance file/mask requires manual review")
        # Survives power loss; removed only after successful replacement or recovery.
        self.ctl("mask", unit)
        self.ctl("daemon-reload")

    def unguard(self, unit):
        self.ctl("unmask", unit)
        self.ctl("daemon-reload")

    def reload(self):
        self.ctl("daemon-reload")

    def boot_state(self, unit):
        boot = self.properties(unit).get("UnitFileState")
        require(boot in ("enabled", "disabled"),
                "Custom/unknown service boot state requires manual review: " + unit)
        return boot

    def disable(self, unit):
        self.ctl("disable", unit)

    def restore_boot(self, uid, unit, boot):
        # Recheck the caller's stock service after unmask/reload. Never use --now.
        own, active = self.inspect(uid)
        require(own == unit and not active, "Cannot restore boot state for a different or active service")
        require(boot in ("enabled", "disabled"), "Invalid saved boot state")
        if self.boot_state(unit) != boot:
            self.ctl("enable" if boot == "enabled" else "disable", unit)
        require(self.boot_state(unit) == boot, "Original boot state was not restored: " + unit)

    def start(self, unit):
        self.ctl("start", unit)
        require(self.properties(unit).get("ActiveState") == "active", "Service failed to remain active")


@contextlib.contextmanager
def runtime_locks():
    # Hold the exact existing abstract lock for every NSS account while files change.
    # An active lock is a conflict, never bypassed or removed.
    with contextlib.ExitStack() as stack:
        for uid in sorted({p.pw_uid for p in pwd.getpwall()}):
            sock = stack.enter_context(socket.socket(socket.AF_UNIX))
            try:
                sock.bind("\0piair.automatic." + str(uid))
            except OSError as error:
                raise RuntimeError(f"Automatic playback/setup lock held for UID {uid}; stop it yourself") from error
        yield


class Installation:
    def __init__(self, root=Path("/")):
        self.root = root

    def path(self, absolute):
        return self.root / str(absolute).lstrip("/")

    def safe(self, path):
        # Refuse symlinks, untrusted parents and nonregular destinations.
        for parent in [path, *path.parents]:
            if parent == self.root.parent:
                break
            if parent.exists() or parent.is_symlink():
                m = parent.lstat()
                require(not stat.S_ISLNK(m.st_mode), "Symlink refused: " + str(parent))
                require(m.st_uid == os.geteuid() and m.st_mode & 0o022 == 0,
                        "Untrusted ownership/permissions: " + str(parent))
        if path.exists():
            require(path.is_file(), "Not a regular file: " + str(path))

    def read_manifest(self):
        path = self.path(MANIFEST)
        self.safe(path)
        if not path.exists():
            require(not any(self.path(p).exists() or self.path(p).is_symlink() for p, _ in FILES.values()),
                    "Existing unmanaged installation; review/remove it manually before installing")
            return None
        manifest = json.loads(path.read_text())
        require(set(manifest) == {"version", "files"} and set(manifest["files"]) == set(FILES),
                "Unexpected installation metadata")
        version(manifest["version"])
        for name, (dest, mode) in FILES.items():
            p = self.path(dest)
            self.safe(p)
            require(p.exists() and digest(p.read_bytes()) == manifest["files"][name] and
                    stat.S_IMODE(p.stat().st_mode) == mode, "Installed file missing or modified: " + str(p))
            require("security.capability" not in os.listxattr(p), "File capabilities refused: " + str(p))
        return manifest

    def atomic(self, path, data, mode):
        self.safe(path)
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o755)
        fd, temporary = tempfile.mkstemp(prefix=".piair-", dir=path.parent)
        try:
            with os.fdopen(fd, "wb") as output:
                output.write(data)
                output.flush()
                os.fchmod(output.fileno(), mode)
                os.fsync(output.fileno())
            os.replace(temporary, path)
            self.sync(path.parent)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)

    @staticmethod
    def sync(path):
        fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)

    def recover(self):
        journal = self.path(JOURNAL)
        if not journal.exists():
            return None
        self.safe(journal / "state.json")
        state_path = journal / "state.json"
        if not state_path.exists():
            # Crash while preparing backups: no installed file has changed yet.
            shutil.rmtree(journal)
            return None
        state = json.loads(state_path.read_text())
        require(set(state) in ({"unit", "old", "masked"}, {"unit", "old", "masked", "boot"}),
                "Invalid recovery journal")
        require("boot" not in state or state["boot"] in ("enabled", "disabled"), "Invalid saved boot state")
        for name, (dest, mode) in FILES.items():
            p = self.path(dest)
            if state["old"]:
                self.atomic(p, (journal / name).read_bytes(), mode)
            else:
                self.safe(p)
                p.unlink(missing_ok=True)
        manifest = self.path(MANIFEST)
        if state["old"]:
            self.atomic(manifest, (journal / "installation.json").read_bytes(), 0o644)
        else:
            manifest.unlink(missing_ok=True)
        # Keep recovery journal until systemd reload/unmask succeeds.
        return state

    def cleanup_completed(self):
        completed = self.path(COMPLETED)
        if completed.exists():
            self.safe(completed / "state.json")
            shutil.rmtree(completed)
            self.sync(self.path(BASE))

    def clear_journal(self):
        # Rename is the commit point. A crash during backup deletion must never
        # leave a rollback journal with only half of its backups remaining.
        self.cleanup_completed()
        os.replace(self.path(JOURNAL), self.path(COMPLETED))
        try:
            self.sync(self.path(BASE))
            self.cleanup_completed()
        except OSError as error:
            print(f"Application files committed; backup cleanup pending: {error}")

    def prepare(self, old, unit, boot=None):
        journal = self.path(JOURNAL)
        require(not journal.exists(), "Interrupted transaction; run piair update to recover first")
        journal.mkdir(mode=0o700, parents=True)
        if old:
            for name, (dest, mode) in FILES.items():
                self.atomic(journal / name, self.path(dest).read_bytes(), mode)
            self.atomic(journal / "installation.json", self.path(MANIFEST).read_bytes(), 0o644)
        state = {"unit": unit, "old": bool(old), "masked": True}
        if boot is not None:
            require(boot in ("enabled", "disabled"), "Invalid saved boot state")
            state["boot"] = boot
        self.atomic(journal / "state.json", json.dumps(state).encode(), 0o600)
        self.sync(journal.parent)  # Persist the journal directory before changing boot links.

    def replace(self, files, v):
        # Every file is replaced by rename; a crash is recovered from durable backups.
        for name, (dest, mode) in FILES.items():
            self.atomic(self.path(dest), files[name], mode)
        manifest = {"version": v, "files": {name: digest(data) for name, data in files.items()}}
        self.atomic(self.path(MANIFEST), json.dumps(manifest, sort_keys=True).encode() + b"\n", 0o644)

    def validate_removal(self):
        allowed = {"audio", "maintain.py", "VERSION", "LICENSE", "installation.json", "maintenance.lock", "transaction"}
        require({p.name for p in self.path(BASE).iterdir()} <= allowed,
                "Unexpected files in PiAir installation; review them manually before uninstall")
        require({p.name for p in self.path(BASE / "audio").iterdir()} == {WAV, "LICENSE.md"},
                "Unexpected audio files; review them manually before uninstall")

    def remove(self):
        self.validate_removal()
        self.read_manifest()  # refuse modifications before deleting anything
        for dest, _ in FILES.values():
            p = self.path(dest)
            p.unlink()
            self.sync(p.parent)
        self.path(MANIFEST).unlink()
        self.sync(self.path(BASE))
        # Only remove empty known directories. Never recurse over installed/user data.
        for path in (BASE / "audio", BASE):
            try:
                self.path(path).rmdir()
            except OSError:
                pass


def restore_services(services, uid, unit, state):
    require(state is None or state["unit"] == unit, "Recovery service differs from the current account")
    try:
        services.reload()
        if state:
            services.unguard(unit)
            if "boot" in state:
                services.restore_boot(uid, unit, state["boot"])
    except Exception as error:
        boot = state.get("boot") if state else None
        boot_warning = (f"Original boot state {boot!r} could not be verified/restored. "
                        if boot is not None else "")
        raise RuntimeError(f"Service recovery failed for {unit}; {boot_warning}"
                           f"Recovery journal retained: {error}") from error


def restart_offer(services, unit):
    if confirm("Start PiAir now? This will not enable boot startup."):
        try:
            services.start(unit)
        except RuntimeError as error:
            print(f"Files installed, but start failed: {error}\nInspect systemctl status {unit}; then run piair start.")


def commit(action, archive=None, v=None, services=None, installation=None):
    require(os.geteuid() == 0, "System installation requires normal sudo authorization")
    os.umask(0o022)
    uid = int(os.environ.get("SUDO_UID", "0"))
    require(uid != 0, "Use sudo from the intended normal user; never run playback as root")
    prerequisites()
    services = services or Services()
    install = installation or Installation()
    install.path(BASE).mkdir(mode=0o755, parents=True, exist_ok=True)
    lockpath = install.path(BASE / "maintenance.lock")
    install.safe(lockpath)
    with lockpath.open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if install.path(JOURNAL).exists():
            require(confirm("Interrupted update found. Restore the previous installation first?"), "Recovery cancelled")
            unit, active = services.inspect(uid, recovery=True)
            require(not active, "Stop the service before recovery")
            state_path = install.path(JOURNAL / "state.json")
            install.safe(state_path)
            if state_path.exists():
                require(json.loads(state_path.read_text()).get("unit") == unit,
                        "Interrupted maintenance belongs to another account; recover as that account")
            with runtime_locks():
                try:
                    state = install.recover()
                except Exception as error:
                    raise RuntimeError("Application recovery incomplete; boot startup has not been restored. "
                                       f"Recovery journal retained: {error}") from error
                restore_services(services, uid, unit, state)
                install.clear_journal() if install.path(JOURNAL).exists() else None
            print("Previous installation restored. Run piair update again to retry.")
            if state and state["old"]:
                restart_offer(services, state["unit"])
            return
        install.cleanup_completed()
        require(action in ("install", "uninstall"), "No interrupted transaction to recover")
        old = install.read_manifest()
        unit, active = services.inspect(uid, removing=action == "uninstall")
        files, boot = None, None
        if action == "install":
            version(v)
            if old:
                require(version(v) >= version(old["version"]), "Downgrade refused")
            # Copy before parsing; a user cannot change bytes between verification and use.
            require(Path(archive).stat().st_size <= 64 * 1024 * 1024, "Archive exceeds size limit")
            data = Path(archive).read_bytes()
            files = unpack(data, v, checksum(v))
            if old and old["version"] == v:
                require(old["files"] == {name: digest(data) for name, data in files.items()},
                        "Same version has different contents; use a new release version")
                print("PiAir is already up to date.")
                return
        else:
            require(old is not None, "No managed PiAir installation found")
            install.validate_removal()
            boot = services.boot_state(unit)
            print("Remove these known PiAir-owned files:")
            for destination, _ in FILES.values():
                print("  " + destination)
            print("  " + str(MANIFEST) + " (installation metadata and maintenance lock)")
            print("Your config.toml, receiver approvals, receiver-state and pairing credentials will remain unchanged.")
            require(confirm(f"Uninstall PiAir and disable {unit}?"), "Uninstall cancelled")
        if active:
            require(confirm(f"{unit} is running. Stop it for {action}?"), "Service stop declined; nothing changed")
            services.quiesce(unit)
        # Journal the original boot state before disabling or touching installed files.
        try:
            install.prepare(old, unit, boot)
            if action == "uninstall":
                services.disable(unit)
            services.guard(unit)
            with runtime_locks():
                services.inspect(uid, removing=action == "uninstall", recovery=True)
                if action == "uninstall":
                    install.remove()
                else:
                    install.replace(files, v)
                services.reload()
                services.unguard(unit)
                install.clear_journal()
        except BaseException as original:
            # Includes Ctrl+C. SIGKILL/power loss leaves the durable journal and mask.
            try:
                with runtime_locks():
                    state = install.recover()
                    restore_services(services, uid, unit, state)
                    if state:
                        install.clear_journal()
            except BaseException as rollback_error:
                boot_warning = "Original boot startup has not been verified/restored. " if boot is not None else ""
                raise RuntimeError(f"{original}; rollback incomplete: {rollback_error}. "
                                   f"{boot_warning}Leave playback stopped and run piair update for recovery; "
                                   "see docs/INSTALL.md") from original
            print("Installation failed; previous application files restored. User data unchanged." if state else
                  "Maintenance finalization failed; inspect installed version and service status. User data unchanged.")
            if old and active:
                restart_offer(services, unit)
            raise
        if action == "uninstall":
            lockpath.unlink(missing_ok=True)
            try:
                install.path(BASE).rmdir()
            except OSError:
                pass
            print("PiAir uninstalled. Personal configuration and credentials were preserved.")
        else:
            print("PiAir installed successfully.\nRun piair setup to configure your audio input and AirPlay receiver.\nAfter setup, run piair to open the main menu.")
            if old and active:
                restart_offer(services, unit)


def authorize(*args):
    require(os.geteuid() != 0, "Run piair maintenance as your normal user")
    require(shutil.which("sudo"), "sudo unavailable; ask your administrator to install PiAir")
    # Fixed installed helper. Never execute a helper supplied by the update archive.
    run_args = ["sudo", "/usr/bin/python3", "-I", str(HELPER), "_commit", *args]
    p = subprocess.run(run_args, check=False)
    require(p.returncode == 0, "Maintenance failed or authorization was unavailable; see the message above")


def main(args):
    require(args, "Usage: maintain.py install VERSION ARCHIVE | update | uninstall")
    if args[0] == "_commit":
        require(len(args) in (2, 4), "Invalid maintenance arguments")
        if len(args) == 2:
            require(args[1] in ("recover", "uninstall"), "Invalid operation")
            commit(args[1])
        else:
            require(args[1] == "install", "Invalid operation")
            commit("install", args[3], args[2])
        return
    require(os.geteuid() != 0, "Run maintenance from your normal account")
    prerequisites()
    if args == ["uninstall"]:
        authorize("uninstall")
    elif args == ["update"]:
        if JOURNAL.exists():
            authorize("recover")
            return
        current = VERSION.read_text().strip()
        version(current)
        print("Installed PiAir: " + current)
        latest = json.loads(fetch(API, 1024 * 1024))
        require(not latest.get("prerelease") and not latest.get("draft"), "No stable release available")
        tag = latest["tag_name"]
        require(tag.startswith("v"), "Invalid official release tag")
        v = tag[1:]
        if version(v) <= version(current):
            print("PiAir is already up to date; older releases will not replace this version.")
            return
        print("Available PiAir: " + v)
        if not confirm("Download and install this official release?"):
            print("Update cancelled.")
            return
        with tempfile.TemporaryDirectory(prefix="piair-update-") as temporary:
            data = fetch(f"{REPO}/releases/download/v{v}/{filename(v)}")
            files = unpack(data, v, checksum(v))
            # Start only --version, as the normal user, before privileged installation.
            binary = Path(temporary) / "piair"
            binary.write_bytes(files["piair"])
            binary.chmod(0o755)
            require(run(str(binary), "--version") == "piair " + v, "New executable startup/version check failed")
            archive = Path(temporary) / filename(v)
            archive.write_bytes(data)
            authorize("install", v, str(archive))
    elif len(args) == 3 and args[0] == "install":
        v, archive = args[1:]
        files = unpack(Path(archive).read_bytes(), v, checksum(v))
        require(run(str(Path("piair").absolute()), "--version") == "piair " + v,
                "Executable startup/version check failed")
        require(confirm("Install PiAir's executable, stock service and bundled audio system-wide?"), "Install cancelled")
        require(shutil.which("sudo"), "sudo unavailable; ask your administrator")
        # First install only: this script came from the archive verified in the documented procedure.
        require(files["maintain.py"] == Path(__file__).read_bytes(), "Installer differs from verified package")
        p = subprocess.run(["sudo", "/usr/bin/python3", "-I", str(Path(__file__).absolute()),
                            "_commit", "install", v, str(Path(archive).absolute())], check=False)
        require(p.returncode == 0, "Installation failed or authorization unavailable")
    else:
        raise RuntimeError("Usage: maintain.py install VERSION ARCHIVE | update | uninstall")


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except (Exception, KeyboardInterrupt) as error:
        print(f"PiAir maintenance: {error or 'cancelled'}. No personal configuration or credentials were changed.", file=sys.stderr)
        sys.exit(1)
