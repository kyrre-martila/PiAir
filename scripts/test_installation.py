"""Rootless installation/release tests: temporary paths and mocked systemd/network."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("piair_maintenance", ROOT / "packaging/maintain.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
spec2 = importlib.util.spec_from_file_location("release", ROOT / "scripts/release.py")
r = importlib.util.module_from_spec(spec2)
spec2.loader.exec_module(r)


def fake_elf(glibc=b"2.36"):
    data = bytearray(64)
    data[:6] = b"\x7fELF\x02\x01"
    struct.pack_into("<H", data, 18, 183)
    return bytes(data) + b"GLIBC_" + glibc + b"\0"


def files(v="0.1.0"):
    return {name: (fake_elf() if name == "piair" else
                   (v + "\n").encode() if name == "VERSION" else
                   (ROOT / "systemd/piair@.service").read_bytes() if name == "piair@.service" else
                   (name + v).encode()) for name in m.FILES}


def archive(content, v="0.1.0", extra=None):
    data = io.BytesIO()
    content = dict(content)
    content["SHA256SUMS"] = "".join(m.digest(value) + "  " + name + "\n"
                                    for name, value in content.items()).encode()
    content.update(extra or {})
    with tarfile.open(fileobj=data, mode="w:gz") as tar:
        for name, value in content.items():
            item = tarfile.TarInfo(f"piair-{v}-aarch64-unknown-linux-gnu/" + name)
            item.size = len(value)
            tar.addfile(item, io.BytesIO(value))
    return data.getvalue()


class FirstInstallExtractionTests(unittest.TestCase):
    """Execute the actual copyable documentation, without sudo or network access."""
    package = "piair-0.1.0-aarch64-unknown-linux-gnu"
    # The first shell block installs OS dependencies; select the download block.
    block = (ROOT / "docs/INSTALL.md").read_text().split("```sh\n")[2].split("```")[0]
    bootstrap = block.split("<<'PYTHON'\n", 1)[1].split("\nPYTHON", 1)[0]

    @staticmethod
    def changed_archive(name, kind=tarfile.REGTYPE, replace=False, repeat=1):
        original = archive(files())
        output = io.BytesIO()
        with tarfile.open(fileobj=io.BytesIO(original), mode="r:gz") as source, \
                tarfile.open(fileobj=output, mode="w:gz") as dest:
            for member in source:
                if not (replace and member.name == name):
                    dest.addfile(member, source.extractfile(member))
            for _ in range(repeat):
                member = tarfile.TarInfo(name)
                member.type = kind
                member.linkname = "/tmp/piair-outside"
                member.size = 0
                dest.addfile(member, io.BytesIO())
        return output.getvalue()

    def bootstrap_in(self, directory, data):
        (directory / (self.package + ".tar.gz")).write_bytes(data)
        return subprocess.run([sys.executable, "-I", "-", self.package], input=self.bootstrap,
                              cwd=directory, text=True, capture_output=True, check=False)

    def test_safe_extraction_writes_only_expected_regular_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            content = files()
            result = self.bootstrap_in(directory, archive(content))
            self.assertEqual(result.returncode, 0, result.stderr)
            extracted = directory / self.package
            actual = {p.relative_to(extracted).as_posix() for p in extracted.rglob("*") if p.is_file()}
            self.assertEqual(actual, set(m.FILES) | {"SHA256SUMS"})
            for name, value in content.items():
                path = extracted / name
                self.assertFalse(path.is_symlink())
                self.assertEqual(path.read_bytes(), value)
                self.assertEqual(path.stat().st_mode & 0o7777, m.FILES[name][1])
            subprocess.run(["sha256sum", "--check", "SHA256SUMS"], cwd=extracted,
                           check=True, capture_output=True)

    def test_malicious_members_rejected_before_any_extraction(self):
        prefix = self.package + "/"
        attacks = [
            ("/tmp/piair-outside", tarfile.REGTYPE, False, 1),
            (prefix + "../outside", tarfile.REGTYPE, False, 1),
            (prefix + "audio/../../outside", tarfile.REGTYPE, False, 1),
            (prefix + "audio/../piair", tarfile.REGTYPE, False, 1),
            (prefix + "unexpected", tarfile.REGTYPE, False, 1),
            (prefix + "./piair", tarfile.REGTYPE, False, 1),
            (prefix + "/piair", tarfile.REGTYPE, False, 1),
            (prefix + "audio", tarfile.SYMTYPE, False, 1),
            (self.package, tarfile.LNKTYPE, False, 1),
            (prefix + "piair", tarfile.SYMTYPE, True, 1),
            (prefix + "piair", tarfile.LNKTYPE, True, 1),
            (prefix + "piair", tarfile.DIRTYPE, True, 1),
            (prefix + "piair", tarfile.FIFOTYPE, True, 1),
            (prefix + "piair", tarfile.CHRTYPE, True, 1),
            (prefix + "piair", tarfile.BLKTYPE, True, 1),
            (prefix + "piair", tarfile.CONTTYPE, True, 1),
            (prefix + "piair", tarfile.REGTYPE, False, 1),
            (prefix + "SHA256SUMS", tarfile.REGTYPE, False, 1),
            (self.package, tarfile.DIRTYPE, False, 2),
            (prefix + "audio", tarfile.DIRTYPE, False, 2),
        ]
        for name, kind, replace, repeat in attacks:
            with self.subTest(name=name, kind=kind), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                data = self.changed_archive(name, kind, replace, repeat)
                result = self.bootstrap_in(directory, data)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual({p.name for p in directory.iterdir()}, {self.package + ".tar.gz"})
                # The installed updater/installer must enforce the same structural policy.
                with self.assertRaises(RuntimeError):
                    m.unpack(data, "0.1.0", m.digest(data))

    def test_missing_files_and_corrupt_tar_write_nothing(self):
        incomplete = files()
        del incomplete["audio/LICENSE.md"]
        for data in (archive(incomplete), b"not a tar archive"):
            with self.subTest(data=data[:16]), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                self.assertNotEqual(self.bootstrap_in(directory, data).returncode, 0)
                self.assertFalse((directory / self.package).exists())

    def test_existing_destination_or_symlink_is_never_followed(self):
        for symlink in (False, True):
            with self.subTest(symlink=symlink), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                outside = directory / "personal-files"
                outside.mkdir()
                (outside / "piair").write_text("preserve me")
                target = directory / self.package
                target.symlink_to(outside, target_is_directory=True) if symlink else target.mkdir()
                result = self.bootstrap_in(directory, archive(files()))
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual((outside / "piair").read_text(), "preserve me")
                self.assertEqual({p.name for p in outside.iterdir()}, {"piair"})

    def test_documented_procedure_cleans_downloads_on_success_or_failure(self):
        for outcome in ("success", "installer failure", "checksum mismatch", "unsafe archive", "download failure"):
            with self.subTest(outcome=outcome), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                binaries = directory / "bin"
                binaries.mkdir()
                scratch = directory / "scratch"
                scratch.mkdir()
                marker = directory / "installer-started"
                content = files()
                # Harmless normal-user installer stand-in; never calls sudo/systemctl.
                content["maintain.py"] = ("import os, pathlib, sys\n"
                    "pathlib.Path(os.environ['PIAIR_TEST_STARTED']).write_text('started')\n"
                    "sys.exit(int(os.environ['PIAIR_TEST_EXIT']))\n").encode()
                data = archive(content)
                if outcome == "unsafe archive":
                    data = self.changed_archive(self.package + "/../outside")
                (directory / (self.package + ".tar.gz")).write_bytes(data)
                sha = "0" * 64 if outcome == "checksum mismatch" else m.digest(data)
                (directory / "SHA256SUMS").write_text(sha + "  " + self.package + ".tar.gz\n")
                (binaries / "mktemp").write_text("#!" + sys.executable + "\n"
                    "import os, tempfile\nprint(tempfile.mkdtemp(dir=os.environ['PIAIR_TEST_SCRATCH']))\n")
                (binaries / "curl").write_text("#!" + sys.executable + "\n"
                    "import os, pathlib, shutil, sys\n"
                    "if os.environ['PIAIR_TEST_DOWNLOAD_FAIL'] == '1': sys.exit(22)\n"
                    "dest = sys.argv[sys.argv.index('-o') + 1]\n"
                    "shutil.copyfile(pathlib.Path(os.environ['PIAIR_TEST_FIXTURE']) / dest, dest)\n")
                for path in binaries.iterdir():
                    path.chmod(0o755)
                env = dict(os.environ, PATH=str(binaries) + os.pathsep + os.environ["PATH"],
                           PIAIR_TEST_SCRATCH=str(scratch), PIAIR_TEST_FIXTURE=str(directory),
                           PIAIR_TEST_STARTED=str(marker),
                           PIAIR_TEST_EXIT="1" if outcome == "installer failure" else "0",
                           PIAIR_TEST_DOWNLOAD_FAIL="1" if outcome == "download failure" else "0")
                result = subprocess.run(["bash", "-c", self.block], cwd=directory, env=env,
                                        text=True, capture_output=True, check=False)
                self.assertEqual(result.returncode == 0, outcome == "success", result.stderr)
                self.assertEqual(list(scratch.iterdir()), [], result.stderr)
                self.assertEqual(marker.exists(), outcome in ("success", "installer failure"))


class ServiceMock:
    def __init__(self, active=False):
        self.active = active
        self.events = []
        self.fail = None
        self.enabled = True

    def inspect(self, uid, **kwargs):
        self.events.append("inspect")
        return "piair@test.service", self.active

    def quiesce(self, unit):
        self.events.append("stop")
        self.active = False

    def guard(self, unit):
        self.events.append("mask")

    def unguard(self, unit):
        self.events.append("unmask")

    def reload(self):
        self.events.append("reload")
        if self.fail:
            error, self.fail = self.fail, None
            raise error

    def disable(self, unit):
        self.events.append("disable")
        self.enabled = False

    def start(self, unit):
        self.events.append("start")
        raise RuntimeError("mock start failed")


class InstallTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.i = m.Installation(self.root)
        self.i.path(m.BASE).mkdir(parents=True)
        self.service = ServiceMock()
        self.personal = self.root / "home/test/.config/piair"
        self.personal.mkdir(parents=True, mode=0o700)
        (self.personal / "config.toml").write_text("custom configuration\n")
        (self.personal / "receiver-state").mkdir(mode=0o700)
        (self.personal / "receiver-state/credentials").write_bytes(b"private pairing material")
        (self.personal / "receiver-state/credentials").chmod(0o600)
        self.before = self.snapshot()

    def snapshot(self):
        return {str(p): (p.read_bytes(), p.stat().st_mode, p.stat().st_uid)
                for p in self.personal.rglob("*") if p.is_file()}

    def seed(self, v="0.1.0"):
        self.i.prepare(None, "piair@test.service")
        self.i.replace(files(v), v)
        self.i.clear_journal()
        return self.i.read_manifest()

    def commit(self, action="install", v="0.2.0", answers=(True, False)):
        content = files(v)
        data = archive(content, v)
        path = self.root / "release.tar.gz"
        path.write_bytes(data)
        with contextlib.ExitStack() as stack:
            for target, value in [("prerequisites", None), ("runtime_locks", contextlib.nullcontext()),
                                  ("checksum", m.digest(data))]:
                stack.enter_context(patch.object(m, target, return_value=value))
            stack.enter_context(patch.object(m, "confirm", side_effect=answers))
            # Only privilege detection is mocked. Files remain owned by the test UID.
            stack.enter_context(patch.dict(os.environ, {"SUDO_UID": "1000"}))
            # safe() must use the actual UID, while commit's root gate sees 0.
            real_safe = self.i.safe
            actual_uid = os.geteuid()
            def safe(p):
                with patch.object(os, "geteuid", return_value=actual_uid):
                    return real_safe(p)
            stack.enter_context(patch.object(self.i, "safe", side_effect=safe))
            stack.enter_context(patch.object(os, "geteuid", return_value=0))
            m.commit(action, str(path), v, self.service, self.i)

    def test_install_permissions_and_no_capabilities_or_boot_enablement(self):
        self.commit()
        for dest, mode in m.FILES.values():
            p = self.i.path(dest)
            self.assertEqual(p.stat().st_mode & 0o7777, mode)
            self.assertNotIn("security.capability", os.listxattr(p))
        self.assertNotIn("start", self.service.events)
        self.assertNotIn("disable", self.service.events)
        self.assertEqual(self.snapshot(), self.before)

    def test_repeated_install_and_current(self):
        self.seed()
        self.commit(v="0.1.0")
        self.assertNotIn("mask", self.service.events)

    def test_unmanaged_existing_installation(self):
        p = self.i.path("/usr/local/bin/piair")
        p.parent.mkdir(parents=True)
        p.write_bytes(b"custom")
        with self.assertRaisesRegex(RuntimeError, "unmanaged"):
            self.i.read_manifest()

    def test_newer_update_keeps_configuration_credentials_and_boot_state(self):
        self.seed()
        self.service.active = True
        self.commit()
        self.assertEqual(self.i.read_manifest()["version"], "0.2.0")
        self.assertEqual(self.snapshot(), self.before)
        self.assertTrue(self.service.enabled)
        self.assertLess(self.service.events.index("stop"), self.service.events.index("mask"))
        self.assertNotIn("start", self.service.events)

    def test_declined_stop_changes_nothing(self):
        old = self.seed()
        self.service.active = True
        with self.assertRaisesRegex(RuntimeError, "stop declined"):
            self.commit(answers=(False,))
        self.assertEqual(self.i.read_manifest(), old)
        self.assertNotIn("mask", self.service.events)

    def test_downgrade_rejected(self):
        old = self.seed("0.2.0")
        with self.assertRaisesRegex(RuntimeError, "Downgrade"):
            self.commit(v="0.1.0")
        self.assertEqual(self.i.read_manifest(), old)

    def test_service_reload_failure_rolls_back(self):
        old = self.seed()
        # Guard is mocked separately, so the reload failure is after replacement.
        self.service.fail = RuntimeError("reload failed")
        with self.assertRaisesRegex(RuntimeError, "reload failed"):
            self.commit()
        self.assertEqual(self.i.read_manifest(), old)
        self.assertEqual(self.snapshot(), self.before)

    def test_interrupted_update_restores_binary_template_and_metadata(self):
        old = self.seed()
        self.i.prepare(old, "piair@test.service")
        self.i.atomic(self.i.path("/usr/local/bin/piair"), b"interrupted", 0o755)
        state = self.i.recover()
        self.assertTrue(state["old"])
        self.assertEqual(self.i.read_manifest(), old)
        self.assertEqual(self.snapshot(), self.before)
        self.i.clear_journal()

    def test_atomic_write_failure_keeps_previous_file(self):
        self.seed()
        p = self.i.path("/usr/local/bin/piair")
        before = p.read_bytes()
        with patch.object(os, "replace", side_effect=OSError("disk failure")):
            with self.assertRaises(OSError):
                self.i.atomic(p, b"new", 0o755)
        self.assertEqual(p.read_bytes(), before)

    def test_failed_restart_reports_without_damaging_install(self):
        self.seed()
        self.service.active = True
        with patch("sys.stdout", new_callable=io.StringIO) as output:
            self.commit(answers=(True, True))
        self.assertIn("start failed", output.getvalue())
        self.assertEqual(self.i.read_manifest()["version"], "0.2.0")

    def test_missing_corrupt_or_unexpected_installed_files_refused(self):
        self.seed()
        p = self.i.path("/usr/local/bin/piair")
        p.write_bytes(b"changed")
        with self.assertRaisesRegex(RuntimeError, "modified"):
            self.i.remove()
        p.unlink()
        with self.assertRaisesRegex(RuntimeError, "missing"):
            self.i.read_manifest()

    def test_uninstall_confirmation_and_data_preservation(self):
        self.seed()
        with self.assertRaisesRegex(RuntimeError, "cancelled"):
            self.commit("uninstall", answers=(False,))
        self.assertTrue(self.i.path("/usr/local/bin/piair").exists())
        self.commit("uninstall", answers=(True,))
        self.assertFalse(self.i.path("/usr/local/bin/piair").exists())
        self.assertEqual(self.snapshot(), self.before)
        self.assertIn("disable", self.service.events)

    def test_uninstall_refuses_unexpected_files(self):
        self.seed()
        self.i.path(m.BASE / "unexpected").write_text("keep")
        with self.assertRaisesRegex(RuntimeError, "Unexpected files"):
            self.i.remove()
        self.assertTrue(self.i.path("/usr/local/bin/piair").exists())

    def test_failure_mid_replacement_rolls_back_every_file(self):
        old = self.seed()
        real_atomic = self.i.atomic
        failed = False
        def atomic(path, data, mode):
            nonlocal failed
            if path == self.i.path("/etc/systemd/system/piair@.service") and not failed:
                failed = True
                raise OSError("interrupted write")
            return real_atomic(path, data, mode)
        with patch.object(self.i, "atomic", side_effect=atomic):
            with self.assertRaisesRegex(OSError, "interrupted write"):
                self.commit()
        self.assertEqual(self.i.read_manifest(), old)
        self.assertEqual(self.snapshot(), self.before)

    def test_crash_during_completed_backup_cleanup_keeps_new_version(self):
        self.seed()
        old = self.i.read_manifest()
        self.i.prepare(old, "piair@test.service")
        self.i.replace(files("0.2.0"), "0.2.0")
        os.replace(self.i.path(m.JOURNAL), self.i.path(m.COMPLETED))
        (self.i.path(m.COMPLETED) / "piair").unlink()
        self.assertIsNone(self.i.recover())
        self.i.cleanup_completed()
        self.assertEqual(self.i.read_manifest()["version"], "0.2.0")

    def test_symlink_refused(self):
        self.seed()
        p = self.i.path("/usr/local/bin/piair")
        p.unlink()
        p.symlink_to(self.personal / "config.toml")
        with self.assertRaisesRegex(RuntimeError, "Symlink"):
            self.i.read_manifest()
        self.assertEqual(self.snapshot(), self.before)


class ReleaseTests(unittest.TestCase):
    def test_version_tag_mismatch_and_invalid_semver(self):
        for tag, v in [("v0.2.0", "0.1.0"), ("0.1.0", "0.1.0"), ("v0.1.0-rc1", "0.1.0")]:
            with self.assertRaises(RuntimeError):
                r.check_tag(tag, v)
        for v in ["01.0.0", "1.0", "1.0.0/../../", "1.0.0-rc1"]:
            with self.assertRaises(RuntimeError):
                m.version(v)

    def test_real_package_layout_wav_license_and_checksums(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / "piair"
            binary.write_bytes(fake_elf())
            current = r.tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
            package = r.package(binary, Path(tmp) / "dist", "v" + current)
            content = m.unpack(package.read_bytes(), current, m.digest(package.read_bytes()))
            self.assertEqual(set(content), set(m.FILES))
            for name in [m.WAV, "LICENSE.md"]:
                self.assertEqual(content["audio/" + name], (ROOT / "tests/assets/audio" / name).read_bytes())

    def test_checksum_mismatch_missing_corrupt_and_unexpected_archive_files(self):
        good = files()
        data = archive(good)
        with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
            m.unpack(data, "0.1.0", "0" * 64)
        del good["audio/LICENSE.md"]
        for data in [archive(good), archive(files(), extra={"../../evil": b"bad"})]:
            with self.assertRaises(RuntimeError):
                m.unpack(data, "0.1.0", m.digest(data))
        with self.assertRaises(tarfile.ReadError):
            m.unpack(b"broken", "0.1.0", m.digest(b"broken"))

    def test_unsupported_architecture_and_glibc(self):
        for data in [b"not ELF", fake_elf(b"2.37"), fake_elf(b"2.9")[:18] + b"\x3e\x00" + fake_elf()[20:]]:
            with self.assertRaises(RuntimeError):
                m.elf(data)
        m.elf(fake_elf())

    def test_missing_alsa_systemd_or_wrong_platform(self):
        with patch.object(m.platform, "system", return_value="Darwin"):
            with self.assertRaisesRegex(RuntimeError, "Linux ARM64"):
                m.prerequisites()
        with patch.object(m.platform, "system", return_value="Linux"), \
                patch.object(m.platform, "machine", return_value="aarch64"), \
                patch.object(m.platform, "libc_ver", return_value=("glibc", "2.36")):
            with patch.object(m.ctypes, "CDLL", side_effect=OSError()):
                with self.assertRaisesRegex(RuntimeError, "ALSA"):
                    m.prerequisites()
            with patch.object(m.ctypes, "CDLL"), patch.object(m.shutil, "which", return_value=None):
                with self.assertRaisesRegex(RuntimeError, "systemd"):
                    m.prerequisites()

    def test_network_failure_and_unavailable_authorization(self):
        with patch.object(m.urllib.request.OpenerDirector, "open", side_effect=OSError("network unavailable")):
            with self.assertRaisesRegex(OSError, "network unavailable"):
                m.fetch(m.API)
        with patch.object(os, "geteuid", return_value=1000), patch.object(m.shutil, "which", return_value=None):
            with self.assertRaisesRegex(RuntimeError, "sudo unavailable"):
                m.authorize("uninstall")

    def test_current_version_does_not_download_or_authorize(self):
        with tempfile.TemporaryDirectory() as tmp:
            v = Path(tmp) / "VERSION"
            v.write_text("0.2.0\n")
            with patch.object(os, "geteuid", return_value=1000), patch.object(m, "prerequisites"), \
                    patch.object(m, "VERSION", v), patch.object(m, "JOURNAL", Path(tmp) / "none"), \
                    patch.object(m, "fetch", return_value=b'{"tag_name":"v0.1.0"}') as fetch, \
                    patch.object(m, "authorize") as auth:
                m.main(["update"])
                self.assertEqual(fetch.call_count, 1)
                auth.assert_not_called()

    def test_new_release_download_verified_before_installed_helper_authorization(self):
        with tempfile.TemporaryDirectory() as tmp:
            v = Path(tmp) / "VERSION"
            v.write_text("0.1.0\n")
            data = archive(files("0.2.0"), "0.2.0")
            with patch.object(os, "geteuid", return_value=1000), patch.object(m, "prerequisites"), \
                    patch.object(m, "VERSION", v), patch.object(m, "JOURNAL", Path(tmp) / "none"), \
                    patch.object(m, "fetch", side_effect=[b'{"tag_name":"v0.2.0"}', data]), \
                    patch.object(m, "checksum", return_value=m.digest(data)), \
                    patch.object(m, "confirm", return_value=True), \
                    patch.object(m, "run", return_value="piair 0.2.0"), patch.object(m, "authorize") as auth:
                m.main(["update"])
                self.assertEqual(auth.call_args.args[:2], ("install", "0.2.0"))

    def test_declined_update_never_downloads(self):
        with tempfile.TemporaryDirectory() as tmp:
            v = Path(tmp) / "VERSION"
            v.write_text("0.1.0\n")
            with patch.object(os, "geteuid", return_value=1000), patch.object(m, "prerequisites"), \
                    patch.object(m, "VERSION", v), patch.object(m, "JOURNAL", Path(tmp) / "none"), \
                    patch.object(m, "fetch", return_value=b'{"tag_name":"v0.2.0"}') as fetch, \
                    patch.object(m, "confirm", return_value=False), patch.object(m, "authorize") as auth:
                m.main(["update"])
                self.assertEqual(fetch.call_count, 1)
                auth.assert_not_called()

    def test_download_failure_or_checksum_failure_never_authorizes(self):
        with tempfile.TemporaryDirectory() as tmp:
            v = Path(tmp) / "VERSION"
            v.write_text("0.1.0\n")
            for result in (OSError("download failed"), b"corrupted"):
                with patch.object(os, "geteuid", return_value=1000), patch.object(m, "prerequisites"), \
                        patch.object(m, "VERSION", v), patch.object(m, "JOURNAL", Path(tmp) / "none"), \
                        patch.object(m, "fetch", side_effect=[b'{"tag_name":"v0.2.0"}', result]), \
                        patch.object(m, "checksum", return_value="0" * 64), \
                        patch.object(m, "confirm", return_value=True), patch.object(m, "authorize") as auth:
                    with self.assertRaises((OSError, RuntimeError)):
                        m.main(["update"])
                    auth.assert_not_called()

    def test_eof_is_not_confirmation_and_no_tty_refused(self):
        with patch.object(sys.stdin, "isatty", return_value=False):
            with self.assertRaisesRegex(RuntimeError, "terminal"):
                m.confirm("change?")
        with patch.object(sys.stdin, "isatty", return_value=True), \
                patch.object(sys.stdout, "isatty", return_value=True), patch("builtins.input", side_effect=EOFError):
            with self.assertRaises(EOFError):
                m.confirm("change?")

    def test_unavailable_sudo_authorization_is_reported(self):
        with patch.object(os, "geteuid", return_value=1000), patch.object(m.shutil, "which", return_value="sudo"), \
                patch.object(m.subprocess, "run") as run:
            run.return_value.returncode = 1
            with self.assertRaisesRegex(RuntimeError, "authorization"):
                m.authorize("uninstall")
            self.assertEqual(run.call_args.args[0][:4], ["sudo", "/usr/bin/python3", "-I", str(m.HELPER)])

    def test_other_account_active_or_custom_service_refused(self):
        svc = m.Services()
        own = "piair@test.service"
        def ctl(*args):
            if args[0] in ("list-units", "list-unit-files"):
                return "piair@other.service loaded active running"
            return ""
        with patch.object(m.pwd, "getpwuid") as pw, patch.object(svc, "ctl", side_effect=ctl), \
                patch.object(svc, "properties", return_value={"ActiveState": "active", "User": "test", "MainPID": "0"}):
            pw.return_value.pw_name = "test"
            with self.assertRaisesRegex(RuntimeError, "Another account"):
                svc.inspect(1000)
        with patch.object(m.pwd, "getpwuid") as pw, patch.object(svc, "ctl", return_value=""), \
                patch.object(svc, "properties", return_value={"ActiveState": "inactive", "User": "test", "DropInPaths": "/custom"}):
            pw.return_value.pw_name = "test"
            with self.assertRaisesRegex(RuntimeError, "Customized"):
                svc.inspect(1000)

    def test_service_guard_survives_reboot_without_changing_boot_enablement(self):
        service = m.Services()
        with patch.object(service, "ctl") as ctl:
            service.guard("piair@piair-isolated-test.service")
            service.unguard("piair@piair-isolated-test.service")
        self.assertEqual([call.args for call in ctl.call_args_list], [
            ("mask", "piair@piair-isolated-test.service"), ("daemon-reload",),
            ("unmask", "piair@piair-isolated-test.service"), ("daemon-reload",)])

    def test_runtime_lock_conflict_is_not_bypassed(self):
        with patch.object(m.socket, "socket") as sock:
            sock.return_value.__enter__.return_value.bind.side_effect = OSError("held")
            with self.assertRaisesRegex(RuntimeError, "lock held"):
                with m.runtime_locks():
                    self.fail("Must not acquire an existing runtime lock")


if __name__ == "__main__":
    unittest.main()
