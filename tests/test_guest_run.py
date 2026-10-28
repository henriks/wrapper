"""Exercise the guest bootstrap shell with isolated HOME and fake tool downloads."""
from concurrent.futures import ThreadPoolExecutor
import fcntl
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
REAL_MISE = shutil.which('mise')

FAKE_MISE = r'''#!/usr/bin/python3
import json, os, pathlib, sys, time
root = pathlib.Path(os.environ['FAKE_ROOT'])
args = sys.argv[1:]
with (root / 'calls.jsonl').open('a') as stream:
    stream.write(json.dumps(args) + '\n')
if os.environ.get('FAKE_MISE_FAIL') == args[0]:
    sys.exit(47)
state_path = root / 'installed.json'
state = json.loads(state_path.read_text()) if state_path.exists() else []
if args[0] == 'ls':
    assert args == ['ls', '--local', '--missing', '--json'], args
    print(json.dumps({name: [{}] for name in json.loads(os.environ.get('FAKE_PROJECT_TOOLS', '[]')) if name not in state}))
elif args[0] == 'which':
    sys.exit(0 if args[1] in state else 1)
elif args[0] in ('use', 'install'):
    if args[0] == 'use' and os.environ.get('FAKE_WAIT_FOR'):
        (root / 'install-started').touch()
        while not pathlib.Path(os.environ['FAKE_WAIT_FOR']).exists():
            time.sleep(0.01)
    tools = [arg for arg in args[1:] if not arg.startswith('-')]
    names = {'node@24': 'node', 'npm:@openai/codex@latest': 'codex',
             'aws-cli@2': 'aws', 'github-cli@latest': 'gh'}
    shims = root / 'home/.local/share/mise/shims'
    shims.mkdir(parents=True, exist_ok=True)
    for tool in tools:
        name = names.get(tool, tool)
        executable = shims / name
        executable.write_text('#!/bin/sh\nprintf "%s\\n" "' + name + ' ready"\n')
        executable.chmod(0o755)
        if name not in state:
            state.append(name)
    temporary = root / ('installed-' + str(os.getpid()))
    temporary.write_text(json.dumps(state))
    temporary.replace(state_path)
elif args[0] == 'activate':
    assert args == ['activate', 'bash', '--shims'], args
else:
    sys.exit('unexpected mise command: ' + repr(args))
'''


class GuestRunTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='guest-run-')
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.home = self.root / 'home'
        self.home.mkdir()
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.project = self.root / 'project'
        self.project.mkdir()
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(('AWS_', 'GH_', 'MISE_', 'XDG_', 'BASH_'))}
        self.env.update(FAKE_ROOT=str(self.root))
        self.executable('mise', FAKE_MISE)
        # The shell under test still passes the real privilege-drop arguments;
        # the host fixture only substitutes the external setpriv operation.
        self.executable('setpriv', '#!/bin/bash\nwhile [[ $1 == --* ]]; do shift; done\nexec "$@"\n')
        self.executable('getent', '#!/bin/sh\necho docker:x:1234:\n')
        source = (ROOT / 'guest/run').read_text()
        source = source.replace('HOME=/home/agent', f'HOME={self.home}')
        source = source.replace('export PATH="', f'export PATH="{self.bin}:', 1)
        self.script = self.root / 'run'
        self.script.write_text(source)

    def executable(self, name, source):
        path = self.bin / name
        path.write_text(source)
        path.chmod(0o755)

    def command(self, *args):
        return ['bash', str(self.script), str(os.getuid()), str(os.getgid()), *args]

    def run_guest(self, *args, **kwargs):
        return subprocess.run(self.command(*args), cwd=self.project, env=self.env,
                              text=True, capture_output=True, timeout=10, **kwargs)

    def calls(self, command=None):
        log = self.root / 'calls.jsonl'
        calls = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
        return [call for call in calls if command is None or call[0] == command]

    def assert_ok(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_fresh_generic_commands_do_not_provision_agent(self):
        self.assert_ok(self.run_guest('/usr/bin/true'))
        result = self.run_guest('bash', '-c', 'printf "%s\\n" "$1"; exit 23', 'bash', 'two words')
        self.assertEqual(result.stdout, 'two words\n')
        self.assertEqual(result.returncode, 23)
        self.assertEqual(self.calls('use'), [])
        self.assertEqual(self.calls('install'), [])
        self.assertEqual(self.calls('which'), [])
        self.assertFalse((self.home / '.cache/sandbox-wrap-tools.lock').exists())

    def test_codex_is_installed_once_and_reused(self):
        for _ in range(2):
            result = self.run_guest('codex', '--version')
            self.assert_ok(result)
            self.assertEqual(result.stdout, 'codex ready\n')
        self.assertEqual(self.calls('use'), [
            ['use', '--global', '--pin', '--yes', 'node@24', 'npm:@openai/codex@latest']])
        self.assertEqual(self.calls('install'), [])

    def test_explicit_credentials_provision_only_requested_clis(self):
        self.env.update(AWS_ACCESS_KEY_ID='test-key', GH_TOKEN='test-token')
        for _ in range(2):
            result = self.run_guest('bash', '-c', 'aws --version; gh --version')
            self.assert_ok(result)
            self.assertEqual(result.stdout, 'aws ready\ngh ready\n')
        self.assertEqual(self.calls('use'), [
            ['use', '--global', '--pin', '--yes', 'aws-cli@2', 'github-cli@latest']])

    def test_project_tools_install_without_global_agent_defaults(self):
        self.env['FAKE_PROJECT_TOOLS'] = '["python"]'
        for _ in range(2):
            result = self.run_guest('python', '--version')
            self.assert_ok(result)
            self.assertEqual(result.stdout, 'python ready\n')
        self.assertEqual(self.calls('install'), [['install', '--yes', 'python']])
        self.assertEqual(self.calls('use'), [])

    def test_generic_attachments_do_not_wait_for_agent_lock(self):
        cache = self.home / '.cache'
        cache.mkdir()
        with (cache / 'sandbox-wrap-tools.lock').open('w') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            with ThreadPoolExecutor(max_workers=4) as pool:
                results = list(pool.map(lambda _: self.run_guest('/usr/bin/true'), range(4)))
            for result in results:
                self.assert_ok(result)
        self.assertEqual(self.calls('use'), [])
        self.assertEqual(self.calls('install'), [])

    def test_concurrent_codex_launches_recheck_after_install(self):
        release = self.root / 'release-install'
        self.env['FAKE_WAIT_FOR'] = str(release)
        first = subprocess.Popen(self.command('codex', '--version'), cwd=self.project, env=self.env,
                                 text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        second = None
        try:
            deadline = time.monotonic() + 5
            while not (self.root / 'install-started').exists():
                self.assertIsNone(first.poll())
                self.assertLess(time.monotonic(), deadline)
                time.sleep(0.01)
            # Generic sessions can execute while Codex provisioning owns the lock.
            self.assert_ok(self.run_guest('/usr/bin/true'))
            second = subprocess.Popen(self.command('codex', '--version'), cwd=self.project, env=self.env,
                                      text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            release.touch()
            for process in (first, second):
                stdout, stderr = process.communicate(timeout=10)
                self.assertEqual(process.returncode, 0, stderr)
                self.assertEqual(stdout, 'codex ready\n')
            self.assertEqual(len(self.calls('use')), 1)
        finally:
            release.touch()
            for process in (first, second):
                if process is not None and process.poll() is None:
                    process.kill()
                    process.communicate()

    def test_project_discovery_failure_stops_before_command(self):
        self.env['FAKE_MISE_FAIL'] = 'ls'
        result = self.run_guest('touch', 'should-not-exist')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.project / 'should-not-exist').exists())

    @unittest.skipUnless(REAL_MISE, 'mise is not installed on the host')
    def test_real_mise_ignores_uninstalled_global_agent_for_generic_command(self):
        (self.bin / 'mise').unlink()
        (self.bin / 'mise').symlink_to(REAL_MISE)
        config = self.home / '.config/mise'
        config.mkdir(parents=True)
        global_config = config / 'config.toml'
        global_config.write_text('[tools]\nnode = "24"\n"npm:@openai/codex" = "latest"\n')
        self.env.update(MISE_OFFLINE='1', MISE_AUTO_UPDATE='0')
        original = global_config.read_text()
        self.assert_ok(self.run_guest('/usr/bin/true'))
        self.assert_ok(self.run_guest('bash', '-c', 'exit 0'))
        self.assertEqual(global_config.read_text(), original)
        self.assertFalse((self.home / '.cache/sandbox-wrap-tools.lock').exists())
        self.assertFalse((self.home / '.local/share/mise/installs').exists())


if __name__ == '__main__':
    unittest.main()
