"""Execute the documented state-preservation shell blocks on private fixtures."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

DOC = (Path(__file__).resolve().parent.parent / "docs/PRE_RELEASE_QA.md").read_text()


class ManualQaStateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.state = self.root / 'config/receiver-state'
        self.state.mkdir(parents=True, mode=0o700)
        self.work = self.root / 'private-backup'
        self.work.mkdir(mode=0o700)
        (self.state / 'selected.toml').write_text('selected fixture')
        self.env = {**os.environ, 'PIAIR_QA_CONFIG': str(self.root / 'config/config.toml'),
                    'PIAIR_QA_WORK': str(self.work)}

    def block(self, name):
        script = re.search(r'```sh\n(# PIAIR_QA_STATE_' + name + r'\n.*?)```', DOC, re.S)
        self.assertIsNotNone(script)
        return subprocess.run(['sh', '-c', script.group(1)], env=self.env,
                              cwd='/', capture_output=True, text=True, check=False)

    def snapshot(self):
        result = self.block('SNAPSHOT')
        self.assertEqual(result.returncode, 0, result.stderr)
        for manifest in self.work.glob('*.sha256'):
            self.assertEqual(manifest.stat().st_mode & 0o077, 0)

    def test_session_only_without_identity_has_valid_empty_credential_baseline(self):
        self.snapshot()
        self.assertEqual((self.work / 'credentials-before.sha256').read_text(), '')
        # First connection may initialize client identity; it is not HAP enrollment.
        (self.state / 'client-identity.toml').write_text('new identity')
        result = self.block('VERIFY')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('No persistent HAP credentials existed at baseline', result.stdout)
        self.assertNotIn('new identity', result.stdout)

    def test_single_stored_hap_credential_is_verified(self):
        (self.state / 'aabbccddeeff.toml').write_text('PRIVATE CREDENTIAL')
        self.snapshot()
        result = self.block('VERIFY')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('aabbccddeeff.toml: OK', result.stdout)
        self.assertNotIn('No persistent HAP credentials', result.stdout)
        self.assertNotIn('PRIVATE CREDENTIAL', result.stdout)

    def test_multiple_existing_credentials_verify_and_detect_missing_or_changed(self):
        names = ['aabbccddeeff.toml', '112233445566.toml']
        for name in names:
            (self.state / name).write_text('PRIVATE CREDENTIAL')
        self.snapshot()
        self.assertEqual(self.block('VERIFY').returncode, 0)
        self.assertNotIn('PRIVATE CREDENTIAL', self.block('VERIFY').stdout)
        for name in names:
            with self.subTest(name=name):
                path = self.state / name
                path.write_text('changed')
                self.assertNotEqual(self.block('VERIFY').returncode, 0)
                path.unlink()
                result = self.block('VERIFY')
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn('No persistent HAP credentials existed', result.stdout)
                path.write_text('PRIVATE CREDENTIAL')
        self.assertEqual(self.block('VERIFY').returncode, 0)

    def test_existing_identity_verified_independently_of_empty_credentials(self):
        identity = self.state / 'client-identity.toml'
        identity.write_text('existing identity')
        self.snapshot()
        self.assertEqual((self.work / 'credentials-before.sha256').stat().st_size, 0)
        self.assertEqual(self.block('VERIFY').returncode, 0)
        identity.write_text('changed identity')
        self.assertNotEqual(self.block('VERIFY').returncode, 0)
        identity.unlink()
        self.assertNotEqual(self.block('VERIFY').returncode, 0)

    def test_selection_change_requires_review_but_missing_selection_fails(self):
        self.snapshot()
        (self.state / 'selected.toml').write_text('explicit new selection')
        result = self.block('VERIFY')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('review privately', result.stdout)
        (self.state / 'selected.toml').unlink()
        self.assertNotEqual(self.block('VERIFY').returncode, 0)

    def test_missing_baseline_is_not_mistaken_for_no_credentials(self):
        self.snapshot()
        (self.work / 'credentials-before.sha256').unlink()
        self.assertNotEqual(self.block('VERIFY').returncode, 0)

    def test_missing_state_directory_is_an_error_including_session_only(self):
        self.snapshot()
        (self.state / 'selected.toml').unlink()
        self.state.rmdir()
        self.assertNotEqual(self.block('SNAPSHOT').returncode, 0)
        self.assertNotEqual(self.block('VERIFY').returncode, 0)
