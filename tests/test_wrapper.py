from contextlib import contextmanager
import importlib.machinery
import importlib.util
import json
import random
import string
import tempfile
from pathlib import Path
import unittest
from unittest.mock import patch

loader = importlib.machinery.SourceFileLoader('wrapper', str(Path(__file__).resolve().parents[1] / 'sandbox-wrap'))
spec = importlib.util.spec_from_loader(loader.name, loader)
w = importlib.util.module_from_spec(spec)
loader.exec_module(w)


@contextmanager
def single_session(project, boot, attach, reset=False):
    (project / '.sandbox').mkdir(exist_ok=True)
    with boot():
        yield


class WrapperTests(unittest.TestCase):
    def test_update_only_when_effective_machine_settings_change(self):
        with tempfile.TemporaryDirectory() as root:
            calls = []
            registered = False
            fail_update = False
            def execute(argv, **kwargs):
                nonlocal registered
                action = argv[2]
                calls.append(action)
                if action == 'create':
                    registered = True
                if action == 'update' and fail_update:
                    raise w.subprocess.CalledProcessError(1, argv)
                return w.subprocess.CompletedProcess(argv, 0, stdout=json.dumps([{'name': 'sandbox'}] if registered else []))
            def launch(flags=(), mounts=('workspace:workspace:rw',)):
                calls.clear()
                with patch.object(w.sys, 'argv', ['sandbox-wrap', '--project', root, *flags, '--', 'true']), \
                     patch.object(w.shutil, 'which', return_value='/bin/smolvm'), \
                     patch.object(w, 'ensure_default_image'), \
                     patch.object(w, 'project_session', side_effect=single_session), \
                     patch.object(w, 'serve_status'), \
                     patch.object(w, 'mount_plan', return_value=list(mounts)), \
                     patch.object(w.subprocess, 'run', side_effect=execute), \
                     patch.object(w.signal, 'signal'):
                    return w.main()
            launch()
            self.assertIn('create', calls)
            launch()
            self.assertNotIn('create', calls)
            self.assertNotIn('update', calls)
            for flags, mounts in (
                (['--no-net'], ['workspace:workspace:rw']),
                (['--no-net', '--port', '8080:80'], ['workspace:workspace:rw']),
                (['--no-net', '--port', '8080:80'], ['workspace:workspace:rw', 'extra:extra:ro']),
                (['--no-net', '--port', '8080:80', '--git'], ['workspace:workspace:rw', 'extra:extra:ro']),
            ):
                launch(flags, mounts)
                self.assertIn('update', calls)
                launch(flags, mounts)
                self.assertNotIn('update', calls)
            path = Path(root) / '.sandbox/launch.json'
            previous = path.read_text()
            fail_update = True
            with self.assertRaises(w.subprocess.CalledProcessError):
                launch()
            self.assertEqual(path.read_text(), previous)
            fail_update = False
            legacy = json.loads(previous)
            del legacy['network']
            path.write_text(json.dumps(legacy))
            launch(flags, mounts)
            self.assertIn('update', calls)
            launch(flags, mounts)
            self.assertNotIn('update', calls)
            del legacy['rpc_socket']
            path.write_text(json.dumps(legacy))
            with self.assertRaisesRegex(SystemExit, 'predates host RPC'):
                launch(flags, mounts)
            self.assertEqual(calls, ['ls'])
            self.assertEqual(json.loads(path.read_text()), legacy)

    def test_image_build_cache_and_explicit_images(self):
        with tempfile.TemporaryDirectory() as root:
            image = Path(root) / 'sandbox-wrap.tar'
            calls = []
            def docker(argv, **kwargs):
                calls.append(argv)
                if argv[1] == 'save':
                    Path(argv[3]).write_bytes(b'complete archive')
            with patch.object(w, 'DEFAULT_IMAGE', image), \
                 patch.object(w.shutil, 'which', return_value='/bin/docker'), \
                 patch.object(w.subprocess, 'run', side_effect=docker):
                w.ensure_default_image('debian:bookworm-slim')
                self.assertEqual(calls, [])
                w.ensure_default_image(str(image))
                self.assertEqual(image.read_bytes(), b'complete archive')
                self.assertEqual([call[1] for call in calls], ['build', 'save'])
                w.ensure_default_image(str(image))
                self.assertEqual(len(calls), 2)
                w.ensure_default_image(str(image), rebuild=True)
                self.assertEqual([call[1] for call in calls], ['build', 'save', 'build', 'save'])

    def test_rebuild_action_does_not_touch_project_or_need_smolvm(self):
        with patch.object(w.sys, 'argv', ['codex-wrap', '--rebuild-image', '--project', '/missing-project']), \
             patch.object(w, 'ensure_default_image') as build, \
             patch.object(w.shutil, 'which', side_effect=AssertionError('must not probe smolvm')):
            w.main()
            build.assert_called_once_with(str(w.DEFAULT_IMAGE), rebuild=True)

    def test_failed_image_export_does_not_publish_partial_archive(self):
        with tempfile.TemporaryDirectory() as root:
            image = Path(root) / 'sandbox-wrap.tar'
            def docker(argv, **kwargs):
                if argv[1] == 'save':
                    Path(argv[3]).write_bytes(b'partial')
                    raise w.subprocess.CalledProcessError(1, argv)
            with patch.object(w, 'DEFAULT_IMAGE', image), \
                 patch.object(w.shutil, 'which', return_value='/bin/docker'), \
                 patch.object(w.subprocess, 'run', side_effect=docker):
                with self.assertRaises(w.subprocess.CalledProcessError):
                    w.ensure_default_image(str(image))
            self.assertFalse(image.exists())
            self.assertFalse(list(Path(root).glob('.sandbox-image-*')))
            image.write_bytes(b'previous valid archive')
            with patch.object(w, 'DEFAULT_IMAGE', image), \
                 patch.object(w.shutil, 'which', return_value='/bin/docker'), \
                 patch.object(w.subprocess, 'run', side_effect=docker):
                with self.assertRaises(w.subprocess.CalledProcessError):
                    w.ensure_default_image(str(image), rebuild=True)
            self.assertEqual(image.read_bytes(), b'previous valid archive')

    def test_credential_output_fuzz(self):
        rng = random.Random(67)
        values = [None, False, 14, [], {}, '', 'key', 'nul\x00key']
        for _ in range(2000):
            value = rng.choice(values)
            if rng.randrange(2):
                value = {key: rng.choice(values) for key in ('AccessKeyId', 'SecretAccessKey', 'SessionToken')}
            encoded = json.dumps(value) if rng.randrange(2) else ''.join(rng.choices(string.printable, k=50))
            try:
                result = w.credential_environment(encoded)
            except ValueError:
                continue
            self.assertTrue(result['AWS_ACCESS_KEY_ID'])
            self.assertTrue(result['AWS_SECRET_ACCESS_KEY'])
            self.assertTrue(all(isinstance(v, str) and '\x00' not in v for v in result.values()))

    def test_secret_values_never_enter_command_arguments(self):
        with tempfile.TemporaryDirectory() as root:
            project = Path(root)
            calls = []
            def execute(argv, **kwargs):
                calls.append((argv, dict(kwargs.get('env', {}))))
                code = 23 if '/usr/local/lib/sandbox-wrap/run' in argv else 0
                return w.subprocess.CompletedProcess(argv, code, stdout='[]')
            with patch.object(w.sys, 'argv', ['sandbox-wrap', '--project', root, '--pass-env', 'TEST_TOKEN', '--', 'echo', 'hello']), \
                 patch.dict(w.os.environ, {'TEST_TOKEN': 'secret-test-value'}), \
                 patch.object(w.shutil, 'which', return_value='/bin/smolvm'), \
                 patch.object(w, 'ensure_default_image'), \
                     patch.object(w, 'project_session', side_effect=single_session), \
                     patch.object(w, 'serve_status'), \
                 patch.object(w, 'mount_plan', return_value=[f'{root}:{root}:rw']), \
                 patch.object(w.subprocess, 'run', side_effect=execute), \
                 patch.object(w.signal, 'signal'):
                self.assertEqual(w.main(), 23)
            for argv, _ in calls:
                self.assertNotIn('secret-test-value', ' '.join(argv))
            session = next((argv, env) for argv, env in calls if '/usr/local/lib/sandbox-wrap/run' in argv)
            self.assertIn('TEST_TOKEN=SANDBOX_WRAP_SECRET_TEST_TOKEN', session[0])
            self.assertEqual(session[1]['SANDBOX_WRAP_SECRET_TEST_TOKEN'], 'secret-test-value')
            self.assertEqual(calls[-1][0][2], 'stop')
            self.assertNotIn('secret-test-value', (project / '.sandbox/launch.json').read_text())

    def test_command_arguments_are_preserved(self):
        rng = random.Random(941)
        # Arbitrary CLI inputs must remain argv elements, never shell source.
        for _ in range(2000):
            command = ['printf', ''.join(rng.choices(string.printable, k=rng.randrange(100)))]
            self.assertEqual(w.parse_args(['--', *command]).command, command)

    def test_active_sessions_inherit_settings_and_reject_changes(self):
        @contextmanager
        def active_session(project, boot, attach, reset=False):
            attach()
            yield
        with tempfile.TemporaryDirectory() as root:
            project = Path(root)
            state = project / '.sandbox'
            state.mkdir()
            codex = project / 'codex'
            codex.mkdir()
            mounts = [f'{root}:{root}:rw', f'{codex}:/home/agent/.codex:rw']
            settings = {'mounts': mounts, 'ports': ['8080:80'], 'git': True, 'network': True,
                        'rpc_socket': str(state / 'host.sock')}
            (state / 'launch.json').write_text(json.dumps(settings))
            def launch(flags):
                with patch.object(w.sys, 'argv', ['codex-wrap', '--project', root, *flags]), \
                     patch.object(w.shutil, 'which', return_value='/bin/smolvm'), \
                     patch.object(w, 'project_session', side_effect=active_session), \
                     patch.object(w.subprocess, 'run', return_value=w.subprocess.CompletedProcess([], 0)) as execute:
                    result = w.main()
                    self.assertEqual(execute.call_count, 1)
                    self.assertEqual(execute.call_args.args[0][-1], 'bash')
                    return result
            for flags in (['--terminal'], ['--terminal', '--git'], ['--terminal', '--port', '8080:80'],
                          ['--terminal', '--codex-home', str(codex)]):
                self.assertEqual(launch(flags), 0)
            for flags in (['--terminal', '--no-net'], ['--terminal', '--port', '9090:80'],
                          ['--terminal', '--codex-home', str(project / 'missing')],
                          ['--terminal', '--ro', str(codex)]):
                with self.assertRaises(SystemExit):
                    launch(flags)
            self.assertEqual(json.loads((state / 'launch.json').read_text()), settings)

    def test_terminal_arguments(self):
        self.assertTrue(w.parse_args(['--terminal']).terminal)
        for args in (['--terminal', '--reset'], ['--terminal', '--rebuild-image'], ['--terminal', '--', 'true']):
            with self.assertRaises(SystemExit):
                w.parse_args(args)

    def test_volume_delimiters_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            for name in ('colon:path', 'line\nbreak', 'return\rbreak'):
                source = Path(root) / name
                source.mkdir()
                with self.assertRaises(SystemExit):
                    w.volume(source, '/safe')

    def test_mounts_cannot_override_protection(self):
        with tempfile.TemporaryDirectory() as root:
            project = Path(root) / 'project'
            project.mkdir()
            child = project / '.sandbox'
            child.mkdir()
            for source in (root, str(project), str(child)):
                args = w.parse_args(['--rw', source])
                with self.assertRaises(SystemExit):
                    w.mount_plan(args, project)

    def test_codex_only_and_readonly_mounts(self):
        with tempfile.TemporaryDirectory() as root:
            project = Path(root) / 'project'
            project.mkdir()
            shared = Path(root) / 'shared'
            shared.mkdir()
            with patch.object(w.Path, 'home', return_value=Path(root)):
                mounts = w.mount_plan(w.parse_args(['--ro', str(shared)]), project)
            self.assertEqual(mounts, [f'{project}:{project}:rw', f'{shared}:{shared}:ro'])

    def test_worktree_metadata_uses_requested_access(self):
        with tempfile.TemporaryDirectory() as root:
            project = Path(root) / 'worktree'
            metadata = Path(root) / 'metadata'
            project.mkdir()
            metadata.mkdir()
            (project / '.git').write_text(f'gitdir: {metadata}')
            result = w.subprocess.CompletedProcess([], 0, stdout=str(metadata))
            with patch.object(w.subprocess, 'run', return_value=result):
                for flags, access in (([], 'ro'), (['--git'], 'rw')):
                    args = w.parse_args(['--codex-home', str(Path(root) / 'missing'), *flags])
                    self.assertIn(f'{metadata}:{metadata}:{access}', w.mount_plan(args, project))


if __name__ == '__main__':
    unittest.main()
