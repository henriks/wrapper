from contextlib import contextmanager
import importlib.machinery
import importlib.util
import json
import io
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
    def setUp(self):
        namespace = patch.object(w.host_mounts, 'ensure_namespace')
        views = patch.object(w.host_mounts, 'prepare', side_effect=lambda project, mounts, git: w.host_mounts.volumes(project / '.sandbox', mounts))
        namespace.start()
        views.start()
        self.addCleanup(namespace.stop)
        self.addCleanup(views.stop)

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
            def launch(flags=('--raw-net',), mounts=('workspace:workspace:rw',)):
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
            path.write_text(previous)
            with self.assertRaisesRegex(SystemExit, 'no proxy bridge'):
                launch(['--unfiltered-net'])
            self.assertEqual(calls, ['ls'])

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
                        'rpc_socket': str(state / 'host.sock'), 'host_mounts': w.host_mounts.LAYOUT}
            (state / 'launch.json').write_text(json.dumps(settings))
            def launch(flags, command=('bash',)):
                with patch.object(w.sys, 'argv', ['codex-wrap', '--project', root, *flags]), \
                     patch.object(w.shutil, 'which', return_value='/bin/smolvm'), \
                     patch.object(w, 'project_session', side_effect=active_session), \
                     patch.object(w.subprocess, 'run', return_value=w.subprocess.CompletedProcess([], 0)) as execute:
                    result = w.main()
                    self.assertEqual(execute.call_count, 1)
                    self.assertEqual(execute.call_args.args[0][-len(command):], list(command))
                    return result
            for flags in (['--entrypoint', 'bash'], ['--entrypoint', 'bash', '--git'], ['--entrypoint', 'bash', '--port', '8080:80'],
                          ['--entrypoint', 'bash', '--codex-home', str(codex)]):
                self.assertEqual(launch(flags), 0)
            command = ('bash', '-lc', 'printf "%s" "a b; $(literal)"')
            self.assertEqual(launch(['--entrypoint', command[0], '--', *command[1:]], command), 0)
            self.assertEqual(launch([], ('codex', '--dangerously-bypass-approvals-and-sandbox')), 0)
            self.assertEqual(launch(['--', 'printf', 'hello world'], ('printf', 'hello world')), 0)
            for flags in (['--entrypoint', 'bash', '--no-net'], ['--entrypoint', 'bash', '--port', '9090:80'],
                          ['--entrypoint', 'bash', '--codex-home', str(project / 'missing')],
                          ['--entrypoint', 'bash', '--ro', str(codex)]):
                with self.assertRaises(SystemExit):
                    launch(flags)
            self.assertEqual(json.loads((state / 'launch.json').read_text()), settings)

    def test_proxy_creation_cli_overrides_and_active_policy(self):
        with tempfile.TemporaryDirectory() as root:
            state = Path(root) / '.sandbox'
            state.mkdir()
            config = {'network': {'mode': 'proxy', 'allow': ['example.com'], 'deny': ['blocked.example.com']}}
            (state / 'config.json').write_text(json.dumps(config))
            calls = []
            def execute(argv, **kwargs):
                calls.append(argv)
                return w.subprocess.CompletedProcess(argv, 0, stdout='[]')
            with patch.object(w.sys, 'argv', ['codex-wrap', '--project', root, '--allow-domain', 'other.org', '--', 'true']), \
                 patch.object(w.shutil, 'which', return_value='/bin/smolvm'), \
                 patch.object(w, 'ensure_default_image'), \
                 patch.object(w, 'project_session', side_effect=single_session), \
                 patch.object(w, 'serve_status'), \
                 patch.object(w.network_proxy, 'serve_proxy'), \
                 patch.object(w.subprocess, 'run', side_effect=execute), \
                 patch.object(w.signal, 'signal'):
                self.assertEqual(w.main(), 0)
            create = next(x for x in calls if x[2] == 'create')
            self.assertNotIn('--net', create)
            self.assertNotIn('--net-backend', create)
            self.assertIn(f'{state / "proxy.sock"}:{w.network_proxy.GUEST_SOCKET}', create)
            self.assertIn('SANDBOX_WRAP_PROXY=1', create)
            saved = json.loads((state / 'launch.json').read_text())
            self.assertEqual(saved['proxy']['allow'], ['other.org'])
            self.assertEqual(saved['proxy']['deny'], ['blocked.example.com'])
            self.assertFalse(saved['network'])
            session = next(x for x in calls if '/usr/local/lib/sandbox-wrap/run' in x)
            self.assertIn('HTTPS_PROXY=http://127.0.0.1:4750', session)
            @contextmanager
            def active(project, boot, attach, reset=False):
                attach()
                yield
            for flags, rejected in (([], False), (['--unfiltered-net'], True), (['--raw-net'], True),
                                    (['--allow-domain', 'other.org'], False),
                                    (['--deny-domain', 'changed.org'], True), (['--no-net'], True)):
                with patch.object(w.sys, 'argv', ['codex-wrap', '--project', root, *flags, '--', 'true']), \
                     patch.object(w.shutil, 'which', return_value='/bin/smolvm'), \
                     patch.object(w, 'project_session', side_effect=active), \
                     patch.object(w.subprocess, 'run', side_effect=execute), \
                     patch.object(w.signal, 'signal'):
                    if rejected:
                        with self.assertRaises(SystemExit):
                            w.main()
                    else:
                        self.assertEqual(w.main(), 0)

    def test_network_defaults_switches_and_configuration_is_read_only(self):
        for flags, mode, allow in (([], 'proxy', list(w.network_proxy.DEFAULT_ALLOW)),
                                   (['--unfiltered-net'], 'proxy', None),
                                   (['--raw-net'], 'direct', None), (['--no-net'], 'off', None)):
            with self.subTest(flags=flags), tempfile.TemporaryDirectory() as root:
                state = Path(root) / '.sandbox'
                state.mkdir()
                text = '{"network":{"deny":["blocked.org"]}}'
                (state / 'config.json').write_text(text)
                calls = []
                def execute(argv, **kwargs):
                    calls.append(argv)
                    return w.subprocess.CompletedProcess(argv, 0, stdout='[]')
                with patch.object(w.sys, 'argv', ['codex-wrap', '--project', root, *flags, '--', 'true']), \
                     patch.object(w.shutil, 'which', return_value='/bin/smolvm'), \
                     patch.object(w, 'ensure_default_image'), \
                     patch.object(w, 'project_session', side_effect=single_session), \
                     patch.object(w, 'serve_status'), patch.object(w.network_proxy, 'serve_proxy'), \
                     patch.object(w.subprocess, 'run', side_effect=execute), patch.object(w.signal, 'signal'):
                    self.assertEqual(w.main(), 0)
                saved = json.loads((state / 'launch.json').read_text())
                self.assertEqual(saved['network'], mode == 'direct')
                self.assertEqual(saved['proxy']['allow'] if saved['proxy'] else None, allow)
                self.assertEqual(saved['proxy']['deny'] if saved['proxy'] else [], ['blocked.org'] if not flags else [])
                self.assertEqual((state / 'config.json').read_text(), text)
                create = next(x for x in calls if x[2] == 'create')
                self.assertEqual('--net' in create, mode == 'direct')
        for flags in (['--raw-net', '--unfiltered-net'], ['--raw-net', '--no-net'],
                      ['--unfiltered-net', '--deny-domain', 'blocked.org'], ['--raw-net', '--allow-domain', 'ok.org']):
            with self.assertRaises(SystemExit):
                w.parse_args(flags)

    def test_config_help_needs_no_project_or_vm(self):
        with patch.object(w.sys, 'argv', ['codex-wrap', '--help-config', '--project', '/missing-project']), \
             patch.object(w.sys, 'stdout', new_callable=io.StringIO) as output, \
             patch.object(w.shutil, 'which', side_effect=AssertionError('must not probe smolvm')):
            w.main()
        self.assertEqual(output.getvalue(), Path(w.__file__).resolve().with_name('config-json.md').read_text())

    def test_reset_preserves_configuration(self):
        with tempfile.TemporaryDirectory() as root:
            state = Path(root) / '.sandbox'
            state.mkdir()
            # Reset must work even when a user is repairing invalid config.
            (state / 'config.json').write_text('invalid')
            (state / 'disk').mkdir()
            (state / 'disk' / 'data').write_text('guest state')
            with patch.object(w.sys, 'argv', ['codex-wrap', '--project', root, '--reset']), \
                 patch.object(w.shutil, 'which', return_value='/bin/smolvm'), \
                 patch.object(w.subprocess, 'run', return_value=w.subprocess.CompletedProcess([], 0, stdout='[]')):
                w.main()
            self.assertEqual(list(state.iterdir()), [state / 'config.json'])
            self.assertEqual((state / 'config.json').read_text(), 'invalid')

    def test_entrypoint_arguments(self):
        self.assertEqual(w.parse_args(['--entrypoint', 'bash']).entrypoint, 'bash')
        args = w.parse_args(['--entrypoint', 'python3', '--', '-c', 'print("hello world")'])
        self.assertEqual(args.entrypoint, 'python3')
        self.assertEqual(args.command, ['-c', 'print("hello world")'])
        for args in (['--entrypoint', 'bash', '--reset'], ['--entrypoint', 'bash', '--rebuild-image'],
                     ['--entrypoint'], ['--terminal']):
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
