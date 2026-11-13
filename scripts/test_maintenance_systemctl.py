"""Exercise the real Services adapter at the subprocess boundary, including failures."""
import os
from pathlib import Path
import shutil
import tempfile
import subprocess
import unittest
from unittest.mock import patch

import test_installation as fixtures

m = fixtures.m


FIELDS = 'LoadState ActiveState User FragmentPath DropInPaths Environment EnvironmentFiles MainPID UnitFileState ExecStart WorkingDirectory'.split()


def record(**changes):
    values = dict.fromkeys(FIELDS, '')
    values.update(LoadState='not-found', ActiveState='inactive', MainPID='0')
    values.update(changes)
    return ''.join(f'{key}={value}\n' for key, value in values.items())


def stock(active=False, **changes):
    values = dict(LoadState='loaded', ActiveState='active' if active else 'inactive',
                  User='test', FragmentPath='/etc/systemd/system/piair@.service',
                  UnitFileState='disabled', WorkingDirectory='~',
                  ExecStart='{ path=/usr/local/bin/piair ; argv[]=/usr/local/bin/piair run --allow-unattended ; ignore_errors=no ; }')
    values.update(changes)
    return record(**values)


class SystemctlTests(unittest.TestCase):
    def invoke(self, output=record(), code=1, inventory='', loaded='', failure=None):
        calls = []
        def execute(args, **kwargs):
            calls.append(args)
            command = args[3]
            if failure and command == failure[0]:
                return subprocess.CompletedProcess(args, *failure[1:])
            if command == 'list-unit-files':
                # Debian 13: filtered zero matches are status 1; unfiltered is 0.
                if 'piair@*.service' in args and not inventory:
                    return subprocess.CompletedProcess(args, 1, '', '')
                return subprocess.CompletedProcess(args, 0, inventory, '')
            if command == 'list-units':
                return subprocess.CompletedProcess(args, 0, loaded, '')
            if command == 'show':
                self.assertIn('--all', args)
                return subprocess.CompletedProcess(args, code, output, '')
            return subprocess.CompletedProcess(args, 0, '', '')
        return execute, calls

    def inspect(self, **kwargs):
        execute, calls = self.invoke(**kwargs)
        with patch.object(m.subprocess, 'run', side_effect=execute), patch.object(m.pwd, 'getpwuid') as pw:
            pw.return_value.pw_name = 'test'
            result = m.Services().inspect(1000)
        return result, calls

    def test_debian13_zero_matches_and_missing_template(self):
        for code in (0, 1):
            result, calls = self.inspect(code=code, inventory='ssh.service enabled enabled\n')
            self.assertEqual(result, ('piair@test.service', False))
            self.assertFalse(any('piair@*.service' in args for args in calls if args[3] == 'list-unit-files'))

    def test_existing_stock_service_inactive_and_active(self):
        for active in (False, True):
            result, _ = self.inspect(output=stock(active), code=0,
                                     inventory='piair@.service disabled enabled\n')
            self.assertEqual(result, ('piair@test.service', active))

    def test_other_account_active(self):
        with self.assertRaisesRegex(RuntimeError, 'Another account'):
            self.inspect(output=stock(True), code=0,
                         loaded='piair@other.service loaded active running PiAir\n')

    def test_custom_service_and_dropins(self):
        for changes in (dict(FragmentPath='/etc/systemd/system/piair@test.service'),
                        dict(DropInPaths='/etc/systemd/system/piair@.service.d/custom.conf'),
                        dict(Environment='CUSTOM=1'), dict(User='root'),
                        dict(ExecStart='/custom'), dict(WorkingDirectory='/tmp')):
            with self.subTest(changes=changes), self.assertRaises(RuntimeError):
                self.inspect(output=stock(**changes), code=0)

    def test_execution_errors_are_never_empty_results(self):
        for command in ('list-units', 'list-unit-files', 'show'):
            for code, stdout, stderr in ((1, '', 'Failed to connect to bus: No such file or directory'),
                                         (1, '', 'Access denied'), (2, '', ''),
                                         (1, '', ''), (1, record(), 'Unexpected error')):
                with self.subTest(command=command, code=code, stderr=stderr), self.assertRaises(RuntimeError):
                    self.inspect(failure=(command, code, stdout, stderr))
        with patch.object(m.subprocess, 'run', side_effect=OSError('execution failed')):
            with self.assertRaises(OSError):
                m.Services().properties('piair@test.service')

    def test_malformed_or_inconsistent_properties_and_inventory(self):
        for output in ('', 'garbage', record() + 'MainPID=0\n', record(LoadState='error'),
                       record(ActiveState='unknown'), record(FragmentPath='/custom'),
                       record(MainPID='123'), record().replace('User=\n', '')):
            with self.subTest(output=output), self.assertRaises(RuntimeError):
                self.inspect(output=output)
        for inventory in ('garbage', 'piair@test.service', 'piair@bad\\x20name.service disabled enabled'):
            with self.subTest(inventory=inventory), self.assertRaises(RuntimeError):
                self.inspect(inventory=inventory)
        with self.assertRaises(RuntimeError):
            self.inspect(output=stock(), code=1)


class FreshInstallTests(unittest.TestCase):
    setUp = fixtures.InstallTests.setUp
    snapshot = fixtures.InstallTests.snapshot
    commit = fixtures.InstallTests.commit
    # Reuse the transaction fixture, but exercise Services rather than ServiceMock.
    def test_fresh_preflight_failure_leaves_no_unmanaged_files_and_retry_works(self):
        execute, calls = SystemctlTests().invoke(failure=('list-unit-files', 1, '', 'Failed to connect to bus'))
        self.service = m.Services()
        with patch.object(m.subprocess, 'run', side_effect=execute), patch.object(m.pwd, 'getpwuid') as pw:
            pw.return_value.pw_name = 'test'
            with self.assertRaisesRegex(RuntimeError, 'Command failed'):
                self.commit()
        self.assertIsNone(self.i.read_manifest())
        self.assertFalse(any(self.i.path(dest).exists() for dest, _ in m.FILES.values()))
        self.assertEqual(self.snapshot(), self.before)
        execute, calls = SystemctlTests().invoke()
        with patch.object(m.subprocess, 'run', side_effect=execute), patch.object(m.pwd, 'getpwuid') as pw:
            pw.return_value.pw_name = 'test'
            self.commit(v='1.0.1')
        self.assertEqual(self.i.read_manifest()['version'], '1.0.1')
        self.assertTrue(all(self.i.path(dest).is_file() for dest, _ in m.FILES.values()))
        self.assertFalse(any(args[3] in ('start', 'enable') for args in calls))
        self.assertEqual(self.snapshot(), self.before)

    def test_fresh_reload_failure_rolls_back_and_retry_works(self):
        self.service.fail = RuntimeError('reload failed')
        with self.assertRaisesRegex(RuntimeError, 'reload failed'):
            self.commit()
        self.assertIsNone(self.i.read_manifest())
        self.assertFalse(any(self.i.path(dest).exists() for dest, _ in m.FILES.values()))
        self.assertEqual(self.snapshot(), self.before)
        self.commit()


class RealSystemctlTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which('systemctl'), 'systemctl unavailable')
    def test_actual_empty_unit_file_inventory(self):
        # --root queries actual systemd's unit-file implementation without a bus
        # or root privileges. On Trixie the filtered command returns 1, not 0.
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'etc/systemd/system').mkdir(parents=True)
            (root / 'etc/systemd/system/unrelated.service').write_text(
                '[Service]\nExecStart=/usr/bin/true\n')
            command = ['systemctl', '--root=' + temporary, '--no-pager',
                       'list-unit-files', '--no-legend', '--plain']
            filtered = subprocess.run(command + ['piair@*.service'], capture_output=True, text=True)
            self.assertIn(filtered.returncode, (0, 1), filtered.stderr)
            self.assertEqual(filtered.stdout, '')
            self.assertEqual(filtered.stderr, '')
            unfiltered = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(unfiltered.returncode, 0, unfiltered.stderr)
            self.assertIn('unrelated.service', unfiltered.stdout)
            self.assertNotIn('piair@', unfiltered.stdout)
            (root / 'etc/systemd/system/piair@.service').write_text(
                '[Service]\nExecStart=/usr/bin/true\n[Install]\nWantedBy=multi-user.target\n')
            installed = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(installed.returncode, 0, installed.stderr)
            self.assertIn('piair@.service', installed.stdout)

    @unittest.skipUnless(Path('/run/systemd/system').is_dir() and os.getuid() != 0,
                         'requires a running system manager and normal user (CI runner)')
    def test_actual_manager_missing_template_and_clean_inspection(self):
        # Read-only integration on the CI runner; never installs or starts a unit.
        services = m.Services()
        self.assertEqual(services.properties('piair@piair-regression-missing.service')['LoadState'], 'not-found')
        own, active = services.inspect(os.getuid())
        self.assertEqual(own, 'piair@' + m.pwd.getpwuid(os.getuid()).pw_name + '.service')
        self.assertFalse(active)
