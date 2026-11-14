"""Exercise the shipped bootstrap with mock network, terminal and maintenance."""
import contextlib
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import types
import unittest
from unittest.mock import patch

from test_installation import ROOT, archive, files, m

b = types.ModuleType('bootstrap')
SOURCE = (ROOT / 'install.sh').read_text().split("<<'PYTHON'\n")[1].split('\nPYTHON')[0]
exec(compile(SOURCE, str(ROOT / 'install.sh'), 'exec'), b.__dict__)


class BootstrapTests(unittest.TestCase):
    def test_stable_selection(self):
        self.assertEqual(b.stable_version(b'{"tag_name":"v2.3.4","draft":false,"prerelease":false}'), '2.3.4')
        for tag in ('v2.0.0-rc1', 'v01.0.0', 'main', 'v1.0.0/evil'):
            with self.subTest(tag=tag), self.assertRaises(RuntimeError):
                b.stable_version(json.dumps(dict(tag_name=tag, draft=False, prerelease=False)))
        for flags in ({'draft': True, 'prerelease': False}, {'draft': False, 'prerelease': True}, {}):
            with self.assertRaises(RuntimeError):
                b.stable_version(json.dumps(dict(tag_name='v1.0.0', **flags)))

    def test_runtime_requirements(self):
        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.object(b.os, 'geteuid', return_value=1000))
            stack.enter_context(patch.object(b.platform, 'system', return_value='Linux'))
            stack.enter_context(patch.object(b.platform, 'libc_ver', return_value=('glibc', '2.36')))
            stack.enter_context(patch.object(b.ctypes, 'CDLL'))
            stack.enter_context(patch.object(b.shutil, 'which', return_value='/usr/bin/tool'))
            stack.enter_context(patch.object(b.pathlib.Path, 'is_dir', return_value=True))
            for arch in ('aarch64', 'armv7l', 'x86_64', 'arm64'):
                with patch.object(b.platform, 'machine', return_value=arch):
                    if arch == 'aarch64':
                        b.prerequisites()
                    else:
                        with self.assertRaisesRegex(RuntimeError, 'ARM64'):
                            b.prerequisites()
            with patch.object(b.platform, 'machine', return_value='aarch64'):
                for target, value, message in (('libc_ver', ('glibc', '2.35'), 'glibc'),
                                               ('libc_ver', ('musl', '1.2'), 'glibc')):
                    with patch.object(b.platform, target, return_value=value), self.assertRaisesRegex(RuntimeError, message):
                        b.prerequisites()
                with patch.object(b.ctypes, 'CDLL', side_effect=OSError()), self.assertRaisesRegex(RuntimeError, 'ALSA'):
                    b.prerequisites()
                with patch.object(b.shutil, 'which', return_value=None), self.assertRaisesRegex(RuntimeError, 'sudo'):
                    b.prerequisites()
                with patch.object(b.pathlib.Path, 'is_dir', return_value=False), self.assertRaisesRegex(RuntimeError, 'systemd'):
                    b.prerequisites()

    def test_exact_checksum(self):
        name = 'piair-1.0.0-aarch64-unknown-linux-gnu.tar.gz'
        good = (m.digest(b'archive') + '  ' + name + '\n').encode()
        b.verify(b'archive', good, name)
        for sums in (b'', good * 2, good.replace(b'  ', b' *'), good.replace(name.encode(), b'other'), b'0' * 64 + b'  ' + name.encode()):
            with self.subTest(sums=sums), self.assertRaises(RuntimeError):
                b.verify(b'archive', sums, name)

    def test_network_errors_limits_and_https_redirects(self):
        with patch.object(b.urllib.request.OpenerDirector, 'open', side_effect=OSError('network failed')):
            with self.assertRaisesRegex(OSError, 'network failed'):
                b.fetch(b.API, 32)
        with patch.object(b.urllib.request.OpenerDirector, 'open') as opened:
            opened.return_value.__enter__.return_value.read.return_value = b'x' * 33
            with self.assertRaisesRegex(RuntimeError, 'size limit'):
                b.fetch(b.API, 32)
        captured = []
        with patch.object(b.urllib.request, 'build_opener', side_effect=lambda handler: captured.append(handler)):
            with self.assertRaises(AttributeError):
                b.fetch(b.API, 32)
        with self.assertRaisesRegex(RuntimeError, 'Non-HTTPS'):
            captured[0].redirect_request(None, None, 302, '', {}, 'http://example.com')

    def exercise(self, outcome):
        content = files('2.3.4')
        data = archive(content, '2.3.4')
        name = 'piair-2.3.4-aarch64-unknown-linux-gnu.tar.gz'
        release = b'{"tag_name":"v2.3.4","draft":false,"prerelease":false}'
        sums = (m.digest(data) + '  ' + name + '\n').encode()
        if outcome == 'checksum':
            sums = b''
        responses = [release, sums, OSError('download failed') if outcome == 'network' else data]
        terminal = unittest.mock.MagicMock()
        terminal.__enter__.return_value.isatty.return_value = True
        original = b.tempfile.TemporaryDirectory
        cwd = Path.cwd()
        with tempfile.TemporaryDirectory() as scratch, contextlib.ExitStack() as stack:
            stack.enter_context(patch.object(b, 'prerequisites'))
            stack.enter_context(patch('builtins.open', return_value=terminal))
            fetch = stack.enter_context(patch.object(b, 'fetch', side_effect=responses))
            stack.enter_context(patch.object(b.tempfile, 'TemporaryDirectory', side_effect=lambda **kw: original(dir=scratch, **kw)))
            def run(args, **kwargs):
                self.assertEqual(Path.cwd().stat().st_mode & 0o777, 0o700)
                self.assertEqual(args[1:4], ['-I', 'maintain.py', 'install'])
                self.assertIs(kwargs['stdin'], terminal.__enter__.return_value)
                if outcome == 'interrupt':
                    b.interrupted(signal.SIGTERM, None)
                return subprocess.CompletedProcess(args, 1 if outcome == 'decline' else 0)
            runner = stack.enter_context(patch.object(b.subprocess, 'run', side_effect=run))
            try:
                if outcome == 'success':
                    b.main()
                else:
                    with self.assertRaises((RuntimeError, OSError, KeyboardInterrupt)):
                        b.main()
                self.assertEqual(list(Path(scratch).iterdir()), [])
                self.assertEqual(runner.call_count, int(outcome in ('success', 'decline', 'interrupt')))
                self.assertTrue(all(call.args[0].startswith('https://') for call in fetch.call_args_list))
            finally:
                os.chdir(cwd)

    def test_cleanup_and_handoff(self):
        for outcome in ('success', 'decline', 'interrupt', 'network', 'checksum'):
            with self.subTest(outcome=outcome):
                self.exercise(outcome)

    def test_no_terminal_fails_before_download(self):
        with patch.object(b, 'prerequisites'), patch('builtins.open', side_effect=OSError('no tty')), patch.object(b, 'fetch') as fetch:
            with self.assertRaises(OSError):
                b.main()
            fetch.assert_not_called()

    def test_shell_is_complete_and_noninteractive_fails_closed(self):
        subprocess.run(['bash', '-n', str(ROOT / 'install.sh')], check=True)
        result = subprocess.run(['bash'], input=(ROOT / 'install.sh').read_bytes(), capture_output=True, start_new_session=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b'PiAir installer:', result.stderr)

    def test_terminal_confirmations_never_treat_empty_as_approval(self):
        for answer in ('', 'n', 'no', 'YES', 'y'):
            with patch.object(m.sys.stdin, 'isatty', return_value=True), patch.object(m.sys.stdout, 'isatty', return_value=True), patch('builtins.input', return_value=answer):
                self.assertEqual(m.confirm('Install?'), answer in ('YES', 'y'))

    def test_piped_bash_prompts_on_controlling_terminal(self):
        import base64
        import fcntl
        import pty
        import select
        import termios
        with tempfile.TemporaryDirectory() as temporary:
            marker = Path(temporary) / 'approved'
            content = files('2.3.4')
            content['maintain.py'] = ("import pathlib, sys, os, signal\n"
                "assert sys.stdin.isatty() and sys.stdout.isatty()\n"
                "print('Approve fixture? [y/N]', flush=True)\n"
                "answer = input().strip().lower()\n"
                f"pathlib.Path({str(marker)!r}).write_text(answer)\n"
                "if answer == 'interrupt': os.kill(os.getppid(), signal.SIGTERM)\n"
                "sys.exit(0 if answer in ('y', 'yes') else 1)\n").encode()
            data = archive(content, '2.3.4')
            sums = (m.digest(data) + '  piair-2.3.4-aarch64-unknown-linux-gnu.tar.gz\n').encode()
            values = [b'{"tag_name":"v2.3.4","draft":false,"prerelease":false}', sums, data]
            injected = "\nimport base64\nprerequisites = lambda: None\nresponses = iter(" + repr([base64.b64encode(v).decode() for v in values]) + ")\nfetch = lambda *args: base64.b64decode(next(responses))\n"
            script = (ROOT / 'install.sh').read_text().replace('if __name__ == "__main__":', injected + '\nif __name__ == "__main__":')
            scratch = Path(temporary) / 'scratch'
            scratch.mkdir()
            script = script.replace('responses = iter(', 'tempfile.tempdir = ' + repr(str(scratch)) + '\nresponses = iter(')
            fixture = Path(temporary) / 'install.sh'
            fixture.write_text(script)
            curl = Path(temporary) / 'curl'
            curl.write_text('#!/bin/sh\ncat "$PIAIR_TEST_SCRIPT"\n')
            curl.chmod(0o755)
            env = dict(os.environ, PATH=temporary + os.pathsep + os.environ['PATH'],
                       PIAIR_TEST_SCRIPT=str(fixture))
            cases = ((mode, answer) for mode in ('pipe', 'wrapper')
                     for answer in (b'\n', b'n\n', b'yes\n', b'interrupt\n'))
            for mode, answer in cases:
                master, slave = pty.openpty()
                def controlling_terminal():
                    os.setsid()
                    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
                try:
                    args = ['bash'] if mode == 'pipe' else [
                        'bash', '-c', FailureReportingTests.documented_command()]
                    child = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=slave, stderr=slave,
                                             preexec_fn=controlling_terminal, pass_fds=(slave,), env=env)
                    if mode == 'pipe':
                        child.stdin.write(script.encode())
                    child.stdin.close()
                    output = b''
                    while b'Approve fixture?' not in output:
                        self.assertTrue(select.select([master], [], [], 10)[0], output)
                        output += os.read(master, 4096)
                    os.write(master, answer)
                    self.assertEqual(child.wait(timeout=10) == 0, answer == b'yes\n')
                    self.assertEqual(marker.read_text(), answer.decode().strip())
                    self.assertEqual(list(scratch.iterdir()), [])
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.wait()
                    os.close(master)
                    os.close(slave)


class FailureReportingTests(unittest.TestCase):
    @staticmethod
    def documented_command():
        readme = (ROOT / 'README.md').read_text().split('```sh\n')[1].split('```')[0].strip()
        install = (ROOT / 'docs/INSTALL.md').read_text().split('```sh\n')[2].split('```')[0].strip()
        if readme != install:
            raise AssertionError('README and INSTALL must document the same tested command')
        return readme

    def test_documented_command_checks_download_before_execution(self):
        command = self.documented_command()
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            curl = directory / 'curl'
            curl.write_text('#!/bin/sh\ncat "$PIAIR_TEST_RESPONSE"\nexit "$PIAIR_TEST_CURL_STATUS"\n')
            curl.chmod(0o755)
            response = directory / 'response'
            marker = directory / 'executed'
            script = 'printf executed > "$PIAIR_TEST_MARKER"\n'
            cases = [('success', script, 0, 0, True),
                     ('installer failure', script + 'exit 7\n', 0, 7, True),
                     ('failed empty download', '', 22, 22, False),
                     ('failed partial download', script, 56, 56, False),
                     ('empty successful response', '', 0, None, False),
                     ('newline-only response', '\n\n', 0, None, False)]
            for label, content, curl_status, expected, executed in cases:
                with self.subTest(case=label):
                    marker.unlink(missing_ok=True)
                    response.write_text(content)
                    env = dict(os.environ, PATH=str(directory) + os.pathsep + os.environ['PATH'],
                               PIAIR_TEST_RESPONSE=str(response), PIAIR_TEST_MARKER=str(marker),
                               PIAIR_TEST_CURL_STATUS=str(curl_status))
                    result = subprocess.run(['bash', '-c', command], env=env, capture_output=True)
                    if expected is None:
                        self.assertNotEqual(result.returncode, 0)
                        self.assertIn(b'PiAir installer download was empty', result.stderr)
                    else:
                        self.assertEqual(result.returncode, expected, result.stderr)
                    self.assertEqual(marker.exists(), executed)
            # Verify the review's failure mode rather than claiming pipe safety.
            response.write_text('')
            env['PIAIR_TEST_CURL_STATUS'] = '22'
            bare = subprocess.run(['bash', '-c', 'curl -fsSL https://example.invalid/install.sh | bash'],
                                  env=env, capture_output=True)
            self.assertEqual(bare.returncode, 0)

    def test_archive_failures_use_top_level_error_handler_without_traceback(self):
        import base64
        from test_installation import FirstInstallExtractionTests
        package = 'piair-2.3.4-aarch64-unknown-linux-gnu'
        incomplete = files('2.3.4')
        del incomplete['audio/LICENSE.md']
        cases = [(archive(files('2.3.4'), '2.3.4', extra={'../escape': b'bad'}), 'Unsafe PiAir archive'),
                 (FirstInstallExtractionTests.changed_archive(package + '/piair', v='2.3.4'), 'Unsafe PiAir archive'),
                 (archive(incomplete, '2.3.4'), 'Incomplete PiAir archive'),
                 (b'not a gzip archive', 'not a gzip file')]
        for data, message in cases:
            with self.subTest(message=message), tempfile.TemporaryDirectory() as temporary:
                name = package + '.tar.gz'
                values = [b'{"tag_name":"v2.3.4","draft":false,"prerelease":false}',
                          (m.digest(data) + '  ' + name + '\n').encode(), data]
                # Exercise real main(), error handling and TemporaryDirectory;
                # replace only platform, terminal, network and destructive handoff.
                injected = ('\nimport base64, contextlib, types\n'
                    'prerequisites = lambda: None\n'
                    'open = lambda *args, **kwargs: contextlib.nullcontext(types.SimpleNamespace(isatty=lambda: True))\n'
                    'responses = iter(' + repr([base64.b64encode(v).decode() for v in values]) + ')\n'
                    'fetch = lambda *args: base64.b64decode(next(responses))\n'
                    'tempfile.tempdir = ' + repr(temporary) + '\n'
                    'def unexpected_handoff(*args, **kwargs):\n'
                    '    raise AssertionError("Unvalidated archive executed")\n'
                    'subprocess.run = unexpected_handoff\n')
                source = SOURCE.replace('if __name__ == "__main__":', injected + '\nif __name__ == "__main__":')
                result = subprocess.run([os.sys.executable, '-I', '-'], input=source, text=True, capture_output=True)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn('PiAir installer: ' + message, result.stderr)
                self.assertIn('No personal configuration or credentials were changed.', result.stderr)
                self.assertNotIn('Traceback', result.stderr)
                self.assertNotIn('Unvalidated archive executed', result.stderr)
                self.assertEqual(list(Path(temporary).iterdir()), [])
