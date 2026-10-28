"""Live host export checks; --mount-only needs no VM and uses disposable data."""
import errno
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import host_mounts
from host_rpc import GUEST_SOCKET, serve_status


def check_prepared_views(root):
    """Run in a child mount namespace; its exit releases every fixture mount."""
    project = root / 'project'; project.mkdir()
    state = project / '.sandbox'; state.mkdir()
    (state / 'config.json').write_text('host control state')
    (project / '.sandbox-wrap.lock').touch()
    (project / '.git').mkdir()
    data = root / 'data'
    private = data / 'parent/private'; private.mkdir(parents=True)
    public = data / 'parent/public'; public.mkdir()
    (public / 'underlying-hidden').write_text('covered host file')
    subprocess.run(['mount', '-t', 'tmpfs', '-o', 'size=1m', 'tmpfs', str(public)], check=True)
    (public / 'intended-export').write_text('visible nested filesystem')
    nested = private / 'nested'; nested.mkdir()
    subprocess.run(['mount', '-t', 'tmpfs', '-o', 'size=1m', 'tmpfs', str(nested)], check=True)
    (nested / 'file').write_text('nested read-only filesystem')
    alias = root / 'project-alias'; alias.mkdir()
    state_alias = root / 'state-alias'; state_alias.mkdir()
    for source, target in ((project, alias), (state, state_alias)):
        subprocess.run(['mount', '--bind', str(source), str(target)], check=True)
    mounts = [f'{project}:/workspace:rw', f'{data}:/data:rw',
              f'{private}:/private:ro', f'{root}:/parent:rw']
    host_mounts.prepare(project, mounts, False)
    exports = state / 'exports'
    for view in (exports / '1', exports / '3/data'):
        assert (view / 'parent/public/intended-export').read_text() == 'visible nested filesystem'
        assert not (view / 'parent/public/underlying-hidden').exists()
    (exports / '1/parent/public/proof').write_text('writable')
    assert (public / 'proof').read_text() == 'writable'

    def fails(operation, expected):
        try:
            operation()
        except OSError as error:
            assert error.errno == expected, error
        else:
            raise AssertionError(f'operation unexpectedly succeeded: {operation}')

    for view in (exports / '0', exports / '3/project', exports / '3/project-alias'):
        assert not (view / '.sandbox/config.json').exists()
        fails(lambda: (view / '.sandbox/config.json').write_text('changed'), errno.EROFS)
        fails(lambda: (view / '.sandbox-wrap.lock').write_text('changed'), errno.EROFS)
        fails(lambda: (view / '.git/forbidden').touch(), errno.EROFS)
    assert not (exports / '3/state-alias/config.json').exists()
    for view in (exports / '1/parent/private', exports / '2', exports / '3/data/parent/private'):
        fails(lambda: (view / 'forbidden').touch(), errno.EROFS)
        assert (view / 'nested/file').read_text() == 'nested read-only filesystem'
        fails(lambda: (view / 'nested/forbidden').touch(), errno.EROFS)
    fails(lambda: (exports / '1/parent').rename(exports / '1/moved'), errno.EBUSY)
    fails(lambda: (exports / '3/project').rename(exports / '3/moved'), errno.EBUSY)
    assert (state / 'config.json').read_text() == 'host control state'
    print('Nested mounts, covered files, aliases, masks, read-only policies and ancestor anchors passed', flush=True)


def mount_only():
    # Keep cleanup outside the child namespace, after all its mounts disappear.
    helper = str(host_mounts.HELPER) if host_mounts.HELPER.exists() else shutil.which('unshare')
    with tempfile.TemporaryDirectory(prefix='wrapper-host-mounts-') as directory:
        subprocess.run([helper, '--user', '--map-current-user', '--mount',
                        '--propagation', 'private', '--keep-caps', sys.executable,
                        str(Path(__file__).resolve()), '--mount-worker', directory],
                       check=True, timeout=60)
        root = Path(directory)
        assert not os.path.ismount(root / 'project/.sandbox/exports')
        assert list((root / 'project/.sandbox/exports').iterdir()) == []
        assert (root / 'data/parent/public/underlying-hidden').read_text() == 'covered host file'


def check_legacy_migration(root, codex):
    project = root / 'legacy'; project.mkdir()
    state = project / '.sandbox'; state.mkdir()
    (state / 'config.json').write_text('{"network":{"mode":"direct"}}')
    env = dict(os.environ, SMOLVM_DATA_DIR=str(state))
    command = [str(ROOT / 'codex-wrap'), '--project', str(project), '--codex-home', str(codex)]
    mounts = [f'{project}:{project}:rw', f'{codex}:/home/agent/.codex:rw']
    def smol(*args):
        subprocess.run(['smolvm', 'machine', *args], env=env, check=True, timeout=120)
    try:
        with serve_status(state / 'host.sock', {'state': 'setup'}):
            smol('create', '--name', 'sandbox', '--image', str(ROOT / 'sandbox-wrap.tar'),
                 '--net', '--net-backend', 'virtio-net', '--volume', '/var/lib/docker',
                 '--volume', mounts[0], '--volume', mounts[1],
                 '--mount-socket', f'{state / "host.sock"}:{GUEST_SOCKET}')
            (state / 'launch.json').write_text(json.dumps({'mounts': mounts, 'ports': [], 'git': False,
                                                         'network': True, 'rpc_socket': str(state / 'host.sock')}))
            smol('start', '--name', 'sandbox')
            smol('exec', '--name', 'sandbox', '--', 'bash', '-ec',
                 'echo persisted > /home/agent/migration-proof; printf "#!/bin/sh\\nexit 99\\n" > /usr/local/lib/sandbox-wrap/prepare; chmod +x /usr/local/lib/sandbox-wrap/prepare')
            smol('stop', '--name', 'sandbox')
        for updated in (True, False):
            result = subprocess.run([*command, '--', 'bash', '-ec', 'test "$(cat ~/migration-proof)" = persisted'],
                                    text=True, capture_output=True, timeout=120)
            assert result.returncode == 0, result.stderr + (state / 'supervisor.log').read_text()
            assert ('Updated machine' in result.stdout) == updated, result.stdout
        assert json.loads((state / 'launch.json').read_text())['host_mounts'] == host_mounts.LAYOUT
        print('Legacy raw mounts migrated once with guest data preserved and current preparation', flush=True)
    finally:
        subprocess.run([*command, '--reset'], check=True, timeout=60)


def main():
    # Fixture bind aliases/nested filesystems need an isolated host namespace.
    # Use a separate marker so wrapper clients still create their own namespaces.
    if 'WRAPPER_MOUNT_FIXTURE_NS' not in os.environ:
        helper = str(host_mounts.HELPER) if host_mounts.HELPER.exists() else shutil.which('unshare')
        os.execvpe(helper, [helper, '--user', '--map-current-user', '--mount',
                          '--propagation', 'private', '--keep-caps', sys.executable, str(Path(__file__).resolve())],
                  dict(os.environ, WRAPPER_MOUNT_FIXTURE_NS='1'))
    with tempfile.TemporaryDirectory(prefix='wrapper-host-mounts-') as directory:
        root = Path(directory)
        project = root / 'project'; project.mkdir()
        state = project / '.sandbox'; state.mkdir()
        config = state / 'config.json'
        contents = '{"network":{"deny":["example.com"]}}'
        config.write_text(contents)
        codex = root / 'codex'; codex.mkdir()
        data = root / 'data'; (data / 'nested/private').mkdir(parents=True)
        public = data / 'nested/public'; public.mkdir()
        (public / 'underlying-hidden').write_text('covered host file')
        readonly = root / 'readonly'; (readonly / 'nested').mkdir(parents=True)
        alias = root / 'project-alias'; alias.mkdir()
        state_alias = root / 'state-alias'; state_alias.mkdir()
        ro_alias = root / 'ro-alias'; ro_alias.mkdir()
        subprocess.run(['git', 'init', '-q', str(project)], check=True)
        fixture_mounts = []
        command = [str(ROOT / 'codex-wrap'), '--project', str(project), '--codex-home', str(codex)]
        flags = ['--rw', str(alias), '--rw', str(data), '--ro', str(data / 'nested/private'), '--ro', str(readonly)]
        def run(script, *options):
            result = subprocess.run([*command, *flags, *options, '--', 'bash', '-ec', script],
                                    text=True, capture_output=True, timeout=150)
            print(result.stdout, end='', flush=True)
            print(result.stderr, end='', file=sys.stderr, flush=True)
            if result.returncode:
                print((state / 'supervisor.log').read_text(), flush=True)
            assert result.returncode == 0, result.returncode
            return result.stdout
        try:
            for source, target in ((project, alias), (state, state_alias)):
                subprocess.run(['mount', '--bind', str(source), str(target)], check=True)
                fixture_mounts.append(target)
            subprocess.run(['mount', '-t', 'tmpfs', '-o', 'size=1m', 'tmpfs', str(public)], check=True)
            fixture_mounts.append(public)
            (public / 'intended-export').write_text('visible nested filesystem')
            subprocess.run(['mount', '-t', 'tmpfs', '-o', 'size=1m', 'tmpfs', str(readonly / 'nested')], check=True)
            fixture_mounts.append(readonly / 'nested')
            (readonly / 'nested/file').write_text('nested')
            subprocess.run(['mount', '--bind', str(readonly / 'nested'), str(ro_alias)], check=True)
            fixture_mounts.append(ro_alias)
            (project / 'attack.sh').write_text('''#!/bin/sh
set -eu
for path in /workspace /alias; do
    test ! -e "$path/.sandbox/config.json"
    test ! -e "$path/.sandbox/launch.json"
    if printf '%s' '{"network":{"mode":"direct"}}' > "$path/.sandbox/config.json"; then exit 1; fi
    if touch "$path/.git/forbidden"; then exit 1; fi
    if ln "$path/.git/config" "$path/git-config-alias"; then exit 1; fi
    if mv "$path/.git" "$path/stolen-git"; then exit 1; fi
    if mv "$path/.sandbox" "$path/stolen-state"; then exit 1; fi
    if rm "$path/.sandbox-wrap.lock"; then exit 1; fi
done
mount -o remount,rw /readonly || true
if touch /readonly/write; then exit 1; fi
test "$(cat /readonly/nested/file)" = nested
if touch /readonly/nested/write; then exit 1; fi
if touch /data/nested/private/write; then exit 1; fi
test "$(cat /data/nested/public/intended-export)" = 'visible nested filesystem'
test ! -e /data/nested/public/underlying-hidden
if mv /data/nested /data/moved; then exit 1; fi
echo workspace > /workspace/proof
echo writable > /data/proof
echo codex > /codex/proof
''')
            output = run(f'''
                docker pull alpine:3.20
                docker run --rm --privileged \\
                    --mount "type=bind,source=$PWD,target=/workspace,bind-recursive=disabled" \\
                    --mount "type=bind,source={alias},target=/alias,bind-recursive=disabled" \\
                    --mount "type=bind,source={data},target=/data,bind-recursive=disabled" \\
                    --mount "type=bind,source={readonly},target=/readonly,bind-recursive=disabled" \\
                    --mount "type=bind,source=/home/agent/.codex,target=/codex,bind-recursive=disabled" \\
                    alpine:3.20 sh /workspace/attack.sh
            ''')
            assert config.read_text() == contents
            assert not (project / '.git/forbidden').exists()
            assert (project / 'proof').read_text().strip() == 'workspace'
            assert (data / 'proof').read_text().strip() == 'writable'
            assert (codex / 'proof').read_text().strip() == 'codex'
            settings = (state / 'launch.json').read_text()
            output = run('true')
            assert 'Updated machine' not in output, output
            assert (state / 'launch.json').read_text() == settings
            run('docker run --rm --privileged --mount "type=bind,source=$PWD,target=/workspace,bind-recursive=disabled" alpine:3.20 touch /workspace/.git/allowed', '--git')
            assert (project / '.git/allowed').exists()
            run('docker run --rm --privileged --mount "type=bind,source=$PWD,target=/workspace,bind-recursive=disabled" alpine:3.20 sh -c "if touch /workspace/.git/forbidden; then exit 1; fi"')
            # Reject alternate host paths into control state or a nested RO filesystem.
            for options in (['--rw', str(state_alias)], ['--rw', str(ro_alias)], ['--codex-home', str(state_alias)]):
                result = subprocess.run([*command, *flags, *options, '--', 'true'], capture_output=True, timeout=30)
                assert result.returncode != 0, options
                log = (state / 'supervisor.log').read_text()
                assert 'control state' in log or 'read-only subtree' in log, log
            assert config.read_text() == contents
            assert not os.path.ismount(state / 'exports')
            assert list((state / 'exports').iterdir()) == []
            # A broad parent export includes the fixture's bind aliases. It
            # must not provide a second route to state or lose earlier masks.
            run('''
                docker run --rm --privileged \
                    --mount type=bind,source=/home/agent/.codex,target=/parent,bind-recursive=disabled \
                    alpine:3.20 sh -ec '
                        for path in /parent/project /parent/project-alias; do
                            test ! -e "$path/.sandbox/config.json"
                            if touch "$path/.git/parent-forbidden"; then exit 1; fi
                        done
                        test ! -e /parent/state-alias/config.json
                        if touch /parent/state-alias/write; then exit 1; fi
                        if touch /parent/ro-alias/write; then exit 1; fi
                        if mv /parent/project /parent/replaced; then exit 1; fi
                    '
            ''', '--codex-home', str(root))
            check_legacy_migration(root, codex)
            print('Host mount policies, privileged bypass rejection, aliases and restart caching passed', flush=True)
        finally:
            subprocess.run([*command, '--reset'], check=True, timeout=60)
            for target in reversed(fixture_mounts):
                subprocess.run(['umount', str(target)], check=True, timeout=10)


if __name__ == '__main__':
    if sys.argv[1:] == ['--mount-only']:
        mount_only()
    elif len(sys.argv) == 3 and sys.argv[1] == '--mount-worker':
        check_prepared_views(Path(sys.argv[2]))
    else:
        main()
