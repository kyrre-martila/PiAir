"""Opt-in real installation on a disposable systemd CI runner only.

Run with sudo, SUDO_UID set to the normal account, and
PIAIR_SYSTEMD_INSTALL_TEST=1. Never run on an existing PiAir installation.
"""
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch

import test_installation as fixtures

m = fixtures.m


@unittest.skipUnless(os.environ.get('PIAIR_SYSTEMD_INSTALL_TEST') == '1' and
                     os.geteuid() == 0 and Path('/run/systemd/system').is_dir(),
                     'opt-in disposable systemd runner installation test')
class FirstInstallSystemdTests(unittest.TestCase):
    def test_real_first_install_rollback_retry_and_inactive_template(self):
        uid = int(os.environ['SUDO_UID'])
        self.assertNotEqual(uid, 0)
        self.assertFalse(m.BASE.exists(), 'requires a completely clean runner')
        for dest, _ in m.FILES.values():
            self.assertFalse(Path(dest).exists() or Path(dest).is_symlink(), dest)
        services = m.Services()
        own, active = services.inspect(uid)
        self.assertFalse(active)
        self.assertEqual(services.properties(own)['LoadState'], 'not-found')

        class FailReloadOnce(m.Services):
            failed = False

            def reload(self):
                if not self.failed:
                    self.failed = True
                    raise RuntimeError('injected post-replacement reload failure')
                super().reload()

        # Hosted runners make these tool directories writable by the runner.
        # Model standard Debian ownership for the test, retaining the production
        # installer's strict parent checks; restore the runner fixture afterward.
        directories = [Path('/usr/local/bin'), Path('/usr/local/share')]
        original = [(p, p.stat()) for p in directories]
        for directory in directories:
            os.chown(directory, 0, 0)
            directory.chmod(0o755)

        # The archive has the real stock template and managed layout. The inert
        # ELF fixture is never executed; ARM64 startup is checked by release CI.
        data = fixtures.archive(fixtures.files('1.0.1'), '1.0.1')
        try:
            with tempfile.TemporaryDirectory() as temporary:
                archive = Path(temporary) / m.filename('1.0.1')
                archive.write_bytes(data)
                # Only hardware prerequisites and remote checksum lookup are
                # substituted. All systemctl commands, locks, writes and rollback
                # execute normally, against the real running system manager.
                with patch.object(m, 'prerequisites'), patch.object(m, 'checksum', return_value=m.digest(data)), \
                        patch.object(m, 'confirm', side_effect=AssertionError('unexpected prompt')):
                    with self.assertRaisesRegex(RuntimeError, 'injected post-replacement'):
                        m.commit('install', archive, '1.0.1', services=FailReloadOnce())
                    self.assertIsNone(m.Installation().read_manifest())
                    for dest, _ in m.FILES.values():
                        self.assertFalse(Path(dest).exists(), dest)
                    self.assertEqual(services.properties(own)['LoadState'], 'not-found')
                    m.commit('install', archive, '1.0.1')
            manifest = m.Installation().read_manifest()
            self.assertEqual(manifest['version'], '1.0.1')
            for dest, mode in m.FILES.values():
                self.assertTrue(Path(dest).is_file(), dest)
                self.assertEqual(Path(dest).stat().st_mode & 0o7777, mode)
            self.assertEqual(services.inspect(uid), (own, False))
            self.assertEqual(services.properties(own)['LoadState'], 'loaded')
            self.assertEqual(services.properties(own)['MainPID'], '0')
            self.assertEqual(services.boot_state(own), 'disabled')
        finally:
            # Every destination was absent before this explicitly opted-in test.
            # Clean only our application files and mask, never a user's home.
            mask = Path('/etc/systemd/system') / own
            if mask.is_symlink() and os.readlink(mask) == '/dev/null':
                services.unguard(own)
            for dest, _ in m.FILES.values():
                Path(dest).unlink(missing_ok=True)
            if m.BASE.exists():
                shutil.rmtree(m.BASE)
            services.reload()
            for directory, before in original:
                os.chown(directory, before.st_uid, before.st_gid)
                directory.chmod(before.st_mode & 0o7777)
