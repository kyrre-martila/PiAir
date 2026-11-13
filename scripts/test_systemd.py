"""Static service contract; no root, service manager, audio or receiver required."""
import configparser
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
UNIT = ROOT / "systemd/piair@.service"


class SystemdTests(unittest.TestCase):
    def setUp(self):
        self.unit = configparser.ConfigParser(interpolation=None, strict=True)
        self.unit.optionxform = str
        self.unit.read(UNIT)

    def test_service_contract(self):
        expected = {
            "Type": "simple", "User": "%i", "WorkingDirectory": "~",
            "ExecStart": "/usr/local/bin/piair run --allow-unattended",
            "ExecStartPre": '/bin/sh -c \'test "$$(/usr/bin/id -u)" -ne 0\'',
            "StandardInput": "null", "StandardOutput": "journal",
            "StandardError": "journal", "Restart": "on-failure",
            "RestartSec": "10s", "KillSignal": "SIGTERM",
            "KillMode": "control-group", "TimeoutStopSec": "30s",
            "AmbientCapabilities": "CAP_NET_BIND_SERVICE",
            "CapabilityBoundingSet": "CAP_NET_BIND_SERVICE",
            "NoNewPrivileges": "yes", "ProtectSystem": "full",
            "ProtectHome": "no", "PrivateTmp": "yes",
            "ProtectKernelTunables": "yes", "ProtectKernelModules": "yes",
            "ProtectControlGroups": "yes", "RestrictSUIDSGID": "yes",
            "UMask": "0077", "PrivateNetwork": "no", "PrivateDevices": "no",
            "RestrictAddressFamilies": "AF_UNIX AF_INET AF_INET6 AF_NETLINK",
        }
        self.assertEqual(dict(self.unit["Service"]), expected)
        self.assertEqual(dict(self.unit["Unit"]), {
            "Description": "PiAir automatic playback for %i",
            "Wants": "network-online.target",
            "After": "network-online.target sound.target",
            "StartLimitIntervalSec": "300", "StartLimitBurst": "3",
        })
        self.assertEqual(dict(self.unit["Install"]), {"WantedBy": "multi-user.target"})
        # No environment/config override, receiver/device default, auth command,
        # PID file, watchdog, private network or competing lock is introduced.
        self.assertEqual(len(self.unit.sections()), 3)
        self.assertEqual(list((ROOT / "systemd").iterdir()), [UNIT])

    @unittest.skipUnless(shutil.which("systemd-analyze"), "systemd-analyze unavailable")
    def test_systemd_verify(self):
        # Verify the actual unit, substituting only the not-yet-installed binary.
        # All sandbox, identity, capability and lifecycle directives stay intact.
        with tempfile.TemporaryDirectory() as directory:
            unit = Path(directory) / UNIT.name
            unit.write_text(UNIT.read_text().replace("/usr/local/bin/piair", "/usr/bin/true"))
            result = subprocess.run(
                ["systemd-analyze", "--user", "--generators=no", "verify", str(unit)],
                env={**os.environ, "XDG_RUNTIME_DIR": directory},
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
