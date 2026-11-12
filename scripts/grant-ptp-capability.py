#!/usr/bin/env python3
"""Explicit Linux deployment step; run with sudo after reviewing this file."""
import os
import shutil
import stat
import subprocess
import sys

CAPABILITY = "cap_net_bind_service=ep"


def open_executable(path):
    """Walk without symlinks and retain an inode for replacement detection."""
    if not path.startswith("/") or any(c.isspace() for c in path):
        raise ValueError("use an absolute executable path without whitespace")
    parts = path.split("/")[1:]
    if not parts or any(p in ("", ".", "..") for p in parts) or parts[-1] != "piair":
        raise ValueError("expected a canonical absolute path ending in piair")
    fd = os.open("/", os.O_RDONLY | os.O_DIRECTORY)
    try:
        for index, part in enumerate(parts):
            parent = os.fstat(fd)
            if parent.st_mode & 0o022:
                raise ValueError("parent directories must not be group/world writable")
            flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
            if index < len(parts) - 1:
                flags |= os.O_DIRECTORY
            child = os.open(part, flags, dir_fd=fd)
            os.close(fd)
            fd = child
        info = os.fstat(fd)
        allowed_owner = int(os.environ.get("SUDO_UID", os.getuid()))
        if (not stat.S_ISREG(info.st_mode)
                or info.st_uid not in (0, allowed_owner)
                or info.st_mode & 0o7022 or not info.st_mode & 0o111):
            raise ValueError("expected an executable owned by root or the invoking user, without set-ID or group/world write bits")
        if os.read(fd, 4) != b"\x7fELF":
            raise ValueError("expected a built ELF PiAir executable, not a script")
        return fd
    except BaseException:
        os.close(fd)
        raise


def capability_tools():
    # Never resolve privileged commands through the invoking user's PATH.
    search = "/usr/sbin:/usr/bin:/sbin:/bin"
    tools = [shutil.which(name, path=search) for name in ("setcap", "getcap")]
    if not all(tools):
        raise ValueError("setcap and getcap are required; install libcap2-bin on Raspberry Pi OS")
    return tools


def grant(target, tools):
    subprocess.run([tools[0], CAPABILITY, target], check=True)
    result = subprocess.run([tools[1], target], check=True,
                            capture_output=True, text=True)
    if result.stdout.strip() != f"{target} {CAPABILITY}":
        raise ValueError("capability verification failed; expected only " + CAPABILITY)


def main():
    try:
        if sys.platform != "linux" or len(sys.argv) != 2:
            raise ValueError("usage (Linux): sudo python3 scripts/grant-ptp-capability.py /absolute/path/to/piair")
        tools = capability_tools()
        fd = open_executable(sys.argv[1])
        try:
            if os.geteuid() != 0:
                raise ValueError("run this installation helper with sudo; run PiAir itself as your normal user")
            grant(sys.argv[1], tools)
            current = os.stat(sys.argv[1], follow_symlinks=False)
            pinned = os.fstat(fd)
            if (current.st_dev, current.st_ino) != (pinned.st_dev, pinned.st_ino):
                raise ValueError("executable was replaced during installation; stop builds and repeat")
        finally:
            os.close(fd)
        print(f"Verified {CAPABILITY} on {sys.argv[1]}; run PiAir as your normal user.")
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"PTP capability installation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
