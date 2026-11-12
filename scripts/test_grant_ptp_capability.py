"""Unprivileged tests: python3 -m unittest discover -s scripts -p 'test_*.py'."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "helper", Path(__file__).with_name("grant-ptp-capability.py"))
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


class HelperTests(unittest.TestCase):
    def test_invalid_paths(self):
        for path in ("piair", "/tmp/../piair", "/tmp//piair", "/tmp/other", "/tmp/a b/piair"):
            with self.subTest(path=path), self.assertRaises(ValueError):
                helper.open_executable(path)

    def test_safe_file_and_unsafe_variants(self):
        # Use a private directory below the checkout: /tmp is deliberately rejected.
        with tempfile.TemporaryDirectory(dir=Path(__file__).resolve().parent) as directory:
            path = Path(directory) / "piair"
            path.write_bytes(b"\x7fELFfixture")
            path.chmod(0o755)
            fd = helper.open_executable(str(path))
            os.close(fd)
            for mode in (0o777, 0o4755, 0o644):
                path.chmod(mode)
                with self.assertRaises(ValueError):
                    helper.open_executable(str(path))
            path.chmod(0o755)
            link = Path(directory) / "hardlink"
            os.link(path, link)
            # Cargo hardlinks the top-level executable to its deps artifact.
            fd = helper.open_executable(str(path))
            os.close(fd)
            link.unlink()
            path.write_text("#!/bin/sh\n")
            with self.assertRaises(ValueError):
                helper.open_executable(str(path))
            path.unlink()
            path.symlink_to("/bin/true")
            with self.assertRaises(OSError):
                helper.open_executable(str(path))
            path.unlink()
            real = Path(directory) / "real"
            real.mkdir()
            path.symlink_to(real, target_is_directory=True)
            with self.assertRaises(OSError):
                helper.open_executable(str(path / "piair"))
            path.unlink()
            Path(directory).chmod(0o777)
            with self.assertRaises(ValueError):
                helper.open_executable(str(path))
            Path(directory).chmod(0o700)

    def test_missing_tools(self):
        with patch.object(helper.shutil, "which", return_value=None):
            with self.assertRaisesRegex(ValueError, "libcap2-bin"):
                helper.capability_tools()

    def test_idempotent_exact_capability_and_verification(self):
        with patch.object(helper.subprocess, "run") as run:
            run.return_value.stdout = "/trusted/piair cap_net_bind_service=ep\n"
            helper.grant("/trusted/piair", ["/usr/sbin/setcap", "/usr/sbin/getcap"])
            helper.grant("/trusted/piair", ["/usr/sbin/setcap", "/usr/sbin/getcap"])
            self.assertEqual(run.call_args_list[0], run.call_args_list[2])
            self.assertEqual(run.call_args_list[0].args[0],
                             ["/usr/sbin/setcap", "cap_net_bind_service=ep", "/trusted/piair"])
            for output in ("", "/trusted/piair cap_net_admin,cap_net_bind_service=ep\n"):
                run.return_value.stdout = output
                with self.assertRaisesRegex(ValueError, "verification failed"):
                    helper.grant("/trusted/piair", ["setcap", "getcap"])
            run.side_effect = subprocess.CalledProcessError(1, "setcap")
            with self.assertRaises(subprocess.CalledProcessError):
                helper.grant("/trusted/piair", ["setcap", "getcap"])


if __name__ == "__main__":
    unittest.main()
