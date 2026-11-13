#!/usr/bin/env python3
"""Build deterministic package layout; never publish here."""
import argparse
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("maintain", ROOT / "packaging/maintain.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


def check_tag(tag, v):
    m.version(v)
    m.require(tag == "v" + v, "Release tag must exactly match Cargo.toml version: v" + v)


def package(binary, out, tag):
    v = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    check_tag(tag, v)
    m.elf(binary.read_bytes())
    sources = {
        "piair": binary,
        "piair@.service": ROOT / "systemd/piair@.service",
        "maintain.py": ROOT / "packaging/maintain.py",
        "LICENSE": ROOT / "LICENSE",
        "audio/" + m.WAV: ROOT / "tests/assets/audio" / m.WAV,
        "audio/LICENSE.md": ROOT / "tests/assets/audio/LICENSE.md",
    }
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as temporary:
        directory = Path(temporary) / m.filename(v).removesuffix(".tar.gz")
        directory.mkdir()
        for name, source in sources.items():
            dest = directory / name
            dest.parent.mkdir(exist_ok=True)
            shutil.copyfile(source, dest)
            dest.chmod(m.FILES[name][1])
        (directory / "VERSION").write_text(v + "\n")
        (directory / "VERSION").chmod(0o644)
        checks = "".join(m.digest((directory / name).read_bytes()) + "  " + name + "\n" for name in sorted(m.FILES))
        (directory / "SHA256SUMS").write_text(checks)
        def normalize(info):
            info.uid = info.gid = 0
            info.uname = info.gname = "root"
            info.mtime = 0
            return info
        archive = out / m.filename(v)
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(directory, arcname=directory.name, filter=normalize)
    data = archive.read_bytes()
    (out / "SHA256SUMS").write_text(m.digest(data) + "  " + archive.name + "\n")
    # Verify the actual archive, including audio and attribution, before upload.
    m.unpack(data, v, m.digest(data))
    subprocess.run(["sha256sum", "--check", "SHA256SUMS"], cwd=out, check=True)
    return archive


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", required=True)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/aarch64-unknown-linux-gnu/release/piair")
    parser.add_argument("--out", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    print(package(args.binary, args.out, args.tag))
