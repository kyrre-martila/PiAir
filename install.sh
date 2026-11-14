#!/usr/bin/env bash
# Run as your ordinary user. Download successfully before executing this script;
# see README.md for the checked one-command bootstrap. The compound command also
# prevents execution of a truncated script.
(
set -eu
command -v python3 >/dev/null || {
  echo 'PiAir: Python 3.9+ required. Run: sudo apt install python3 ca-certificates curl libasound2' >&2
  exit 1
}
python3 -I - <<'PYTHON'
"""Small HTTPS bootstrap; package maintenance owns all system changes."""
import ctypes
import hashlib
import io
import json
import os
import pathlib
import platform
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

REPO = "https://github.com/kyrre-martila/PiAir"
API = "https://api.github.com/repos/kyrre-martila/PiAir/releases/latest"


def require(ok, message):
    if not ok:
        raise RuntimeError(message)


def prerequisites():
    require(os.geteuid() != 0, "Run this installer as your ordinary user, without sudo")
    require(sys.version_info >= (3, 9), "Python 3.9+ required: sudo apt install python3")
    require(platform.system() == "Linux" and platform.machine() == "aarch64",
            "Linux ARM64 required (uname -m: aarch64); use a 64-bit Raspberry Pi OS")
    libc, release = platform.libc_ver()
    require(libc == "glibc" and tuple(map(int, release.split(".")[:2])) >= (2, 36),
            "glibc >= 2.36 required; use Raspberry Pi OS Bookworm / Debian 12 or newer")
    try:
        ctypes.CDLL("libasound.so.2")
    except OSError as error:
        raise RuntimeError("ALSA missing: sudo apt install libasound2 (or libasound2t64)") from error
    require(shutil.which("sudo"), "sudo missing: ask your system administrator to install sudo")
    require(shutil.which("systemctl") and pathlib.Path("/run/systemd/system").is_dir(),
            "A running systemd system manager is required")


def fetch(url, limit):
    class HTTPSOnly(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, req, fp, code, msg, headers, newurl):
            require(newurl.startswith("https://"), "Non-HTTPS redirect refused")
            return super().redirect_request(req, fp, code, msg, headers, newurl)
    request = urllib.request.Request(url, headers={"User-Agent": "PiAir-installer"})
    with urllib.request.build_opener(HTTPSOnly()).open(request, timeout=30) as response:
        data = response.read(limit + 1)
    require(len(data) <= limit, "Release download exceeds size limit")
    return data


def stable_version(data):
    release = json.loads(data)
    require(release.get("draft") is False and release.get("prerelease") is False,
            "No published stable release available")
    tag = release.get("tag_name", "")
    require(re.fullmatch(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", tag),
            "Invalid stable release tag")
    return tag[1:]


def verify(data, sums, name):
    lines = [line for line in sums.decode("ascii").splitlines()
             if line.split() and line.split()[-1].lstrip("*") == name]
    require(len(lines) == 1 and re.fullmatch(r"[0-9a-f]{64}  " + re.escape(name), lines[0]),
            "Selected archive checksum missing or ambiguous")
    require(hashlib.sha256(data).hexdigest() == lines[0][:64], "Release checksum mismatch")


def extract(data, package):
    require(re.fullmatch(r"piair-[0-9]+\.[0-9]+\.[0-9]+-aarch64-unknown-linux-gnu", package),
            "Invalid PiAir package name")
    files = {package + "/" + name for name in (
        "piair", "piair@.service", "maintain.py", "VERSION", "LICENSE", "SHA256SUMS",
        "audio/Pachelbel_Canon_audio_test.wav", "audio/LICENSE.md")}
    directories = {package, package + "/audio"}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        members, seen = archive.getmembers(), set()
        for member in members:
            regular = member.type in (tarfile.REGTYPE, tarfile.AREGTYPE)
            allowed = (member.name in files and regular and 0 <= member.size <= 48 * 1024 * 1024
                       or member.name in directories and member.isdir())
            if not allowed or member.name in seen:
                raise RuntimeError("Unsafe PiAir archive: " + member.name)
            seen.add(member.name)
        if not files <= seen:
            raise RuntimeError("Incomplete PiAir archive")
        pathlib.Path(package).mkdir(mode=0o700)
        for member in members:
            if member.name in files:
                destination = pathlib.Path(member.name)
                destination.parent.mkdir(parents=True, exist_ok=True)
                with destination.open("xb") as output:
                    output.write(archive.extractfile(member).read())
                destination.chmod(0o755 if destination.name == "piair" else 0o644)


def interrupted(signum, frame):
    raise KeyboardInterrupt("interrupted")


def main():
    prerequisites()
    # Open an actual controlling terminal before any download. stdin contains the
    # Python source when piped through bash, never an answer to a prompt.
    with open("/dev/tty", "r+b", buffering=0) as terminal:
        require(terminal.isatty(), "An interactive terminal is required")
        v = stable_version(fetch(API, 1024 * 1024))
        package = "piair-" + v + "-aarch64-unknown-linux-gnu"
        name = package + ".tar.gz"
        print("Downloading official stable PiAir " + v, flush=True)
        base = REPO + "/releases/download/v" + v + "/"
        sums = fetch(base + "SHA256SUMS", 4096)
        data = fetch(base + name, 64 * 1024 * 1024)
        verify(data, sums, name)
        with tempfile.TemporaryDirectory(prefix="piair-install-") as temporary:
            os.chdir(temporary)
            pathlib.Path(name).write_bytes(data)
            extract(data, package)
            # Only checksum-verified code executes, as the normal user. The
            # maintenance helper repeats full validation before sudo operations.
            result = subprocess.run([sys.executable, "-I", "maintain.py", "install", v,
                                     "../" + name], cwd=package,
                                    stdin=terminal, stdout=terminal, stderr=terminal,
                                    check=False)
            require(result.returncode == 0, "Installation cancelled or failed; see terminal output above")


if __name__ == "__main__":
    for sig in (signal.SIGHUP, signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, interrupted)
    try:
        main()
    except (Exception, KeyboardInterrupt) as error:
        print("PiAir installer: " + str(error or "cancelled") +
              ". Check network access, CA certificates and the message above. "
              "No personal configuration or credentials were changed.", file=sys.stderr)
        sys.exit(1)
PYTHON
)
