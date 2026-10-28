"""Real KVM gate; all shared credentials and project files are disposable."""
import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
CODEX_STARTUP = '''
    codex --version
    codex app-server daemon start
    trap 'codex app-server daemon stop' EXIT
    codex app-server daemon version
'''


def check_worktrees(base, codex, project):
    subprocess.run(['git', '-C', str(project), '-c', 'user.name=Test', '-c',
                    'user.email=test@example.invalid', 'commit', '--allow-empty', '-qm', 'initial'], check=True)
    worktree = base / 'worktree'
    subprocess.run(['git', '-C', str(project), 'worktree', 'add', '-q', '-b', 'test', str(worktree)], check=True)
    command = [str(ROOT / 'sandbox-wrap'), '--project', str(worktree), '--codex-home', str(codex)]
    try:
        subprocess.run([*command, '--', 'bash', '-ec',
                        'git status --porcelain; ! touch "$(git rev-parse --git-dir)/forbidden"; ! echo broken > .git'],
                       check=True, timeout=300)
        subprocess.run([*command, '--git', '--', 'bash', '-ec',
                        'git -c user.name=Test -c user.email=test@example.invalid commit --allow-empty -qm writable'],
                       check=True, timeout=120)
        subprocess.run([*command, '--', 'bash', '-ec',
                        'test "$(git log -1 --format=%s)" = writable; ! touch "$(git rev-parse --git-dir)/forbidden"'],
                       check=True, timeout=120)
        print('Linked worktree RO/RW/RO transitions passed', flush=True)
    finally:
        subprocess.run([*command, '--reset'], check=True, timeout=120)


def check_terminal(command, action):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 80, 0, 0))
    trap = "trap 'stty size; exit 0' WINCH" if action == 'resize' else "trap 'exit 130' INT"
    script = trap + '; echo PTY_READY; while :; do sleep 1; done'
    process = subprocess.Popen([*command, '--', 'bash', '-c', script],
                               stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
    os.close(slave)
    output = b''
    deadline = time.monotonic() + 120
    try:
        while b'PTY_READY' not in output and time.monotonic() < deadline:
            if select.select([master], [], [], 1)[0]:
                output += os.read(master, 65536)
        assert b'PTY_READY' in output, output.decode(errors='replace')
        if action == 'resize':
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 35, 100, 0, 0))
            os.killpg(process.pid, signal.SIGWINCH)
        else:
            os.write(master, b'\x03')
        # Drain PTY output while waiting so shutdown cannot block on a full PTY.
        while process.poll() is None and time.monotonic() < deadline:
            if select.select([master], [], [], 1)[0]:
                try:
                    output += os.read(master, 65536)
                except OSError:
                    break
        assert process.wait(timeout=30) == (0 if action == 'resize' else 130), output.decode(errors='replace')
        if action == 'resize':
            assert b'35 100' in output, output.decode(errors='replace')
        print(f'Terminal {action} passed', flush=True)
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=120)
        os.close(master)


def main():
    with tempfile.TemporaryDirectory(prefix='wrapper-live-') as directory:
        base = Path(directory)
        project = base / 'project'
        project.mkdir()
        codex = base / 'codex'
        codex.mkdir()
        (codex / 'test-state').write_text('shared')
        binaries = base / 'bin'
        binaries.mkdir()
        for name, script in {
            'aws': '#!/bin/sh\nif [ "$2" = export-credentials ]; then echo \'{"AccessKeyId":"test-aws-key","SecretAccessKey":"test-aws-secret","SessionToken":"test-aws-session"}\'; else echo eu-north-1; fi\n',
            'gh': '#!/bin/sh\necho test-gh-token\n',
        }.items():
            path = binaries / name
            path.write_text(script)
            path.chmod(0o755)
        environment = dict(os.environ, PATH=f'{binaries}:{os.environ["PATH"]}', WRAPPER_TEST_SECRET='test-secret-value')
        # Host configuration paths must not become guest configuration implicitly.
        environment.update({key: str(base / 'host-only-config') for key in (
            'GIT_CONFIG_GLOBAL', 'AWS_CONFIG_FILE', 'AWS_SHARED_CREDENTIALS_FILE', 'KUBECONFIG', 'SSH_AUTH_SOCK')})
        subprocess.run(['git', 'init', '-q', str(project)], check=True)
        command = [str(ROOT / 'sandbox-wrap'), '--project', str(project), '--codex-home', str(codex)]
        if sys.argv[1:] == ['worktree']:
            check_worktrees(base, codex, project)
            return
        if sys.argv[1:] == ['codex']:
            try:
                subprocess.run([*command, '--entrypoint', 'codex', '--', '--version'], check=True, timeout=300)
                subprocess.run([*command, '--', 'bash', '-ec', CODEX_STARTUP], check=True, timeout=300)
            finally:
                subprocess.run([*command, '--reset'], check=True, timeout=120)
            return

        def run(script, *flags, expected=0, input=None, expected_update=None):
            result = subprocess.run([*command, *flags, '--', 'bash', '-ec', script],
                                    env=environment, input=input, text=True, timeout=300,
                                    stdout=subprocess.PIPE if expected_update is not None else None)
            assert result.returncode == expected, result.returncode
            if expected_update is not None:
                print(result.stdout, end='', flush=True)
                assert ('Updated machine' in result.stdout) == expected_update, result.stdout
                assert 'Created machine' not in result.stdout, result.stdout

        try:
            # Generic commands work before any agent tools have been downloaded.
            subprocess.run([*command, '--no-net', '--', '/usr/bin/true'], check=True, timeout=120)
            subprocess.run([*command, '--no-net', '--entrypoint', 'bash', '--', '-ec',
                            '! command -v codex; ! command -v node'], check=True, timeout=120)
            subprocess.run([*command, '--reset'], check=True, timeout=120)
            (project / '.sandbox/config.json').write_text('{"network":{"allow":[]}}')
            subprocess.run([*command, '--entrypoint', 'bash', '--', '-ec',
                            '! command -v codex; ! command -v node'], check=True, timeout=120)
            (project / '.sandbox/config.json').unlink()
            subprocess.run([*command, '--entrypoint', 'codex', '--', '--version'], check=True, timeout=300)
            run(CODEX_STARTUP)
            run('''
                codex --version
                hostctl status > rpc-status.json
                python3 -c 'import json, os; s=json.load(open("rpc-status.json")); assert s["state"] == "running" and s["project"] == os.getcwd() and not s["network"] and s["proxy"] and not s["git_writable"] and s["wrapper_pid"] > 0'
                test "$(cat ~/.codex/test-state)" = shared
                test ! -e .sandbox/launch.json
                ! touch .git/forbidden
                echo persisted > ~/persistent-test
                test -z "${GIT_CONFIG_GLOBAL+x}${AWS_CONFIG_FILE+x}${AWS_SHARED_CREDENTIALS_FILE+x}${KUBECONFIG+x}${SSH_AUTH_SOCK+x}"
                git config --global user.name guest-review-user
                git config --global user.email guest-review@example.invalid
                mkdir -p ~/.aws ~/.kube
                printf '[default]\nregion = eu-west-1\n' > ~/.aws/config
                printf '[default]\naws_access_key_id = guest-key\naws_secret_access_key = guest-secret\n' > ~/.aws/credentials
                echo guest-kube-config > ~/.kube/config
                echo shared > workspace-test
                docker version
                docker compose version
                docker buildx version
                docker volume create wrapper-test
                docker pull alpine:3.20
                printf 'FROM alpine:3.20\nRUN echo built-inside-vm > /build-test\n' | docker build --network=none -t wrapper-built -
                docker run --rm wrapper-built cat /build-test
                docker run --rm -v wrapper-test:/data alpine:3.20 sh -c 'echo docker-persisted > /data/test'
                test "$DOCKER_HOST" = unix:///run/docker.sock
                docker run --rm -v "$PWD:/workspace" alpine:3.20 sh -c 'cat /workspace/workspace-test; echo container-write > /workspace/container-test'
                curl --fail --max-time 30 https://pypi.org/simple/pip/ >/dev/null
            ''', expected_update=False)
            assert (project / 'workspace-test').read_text().strip() == 'shared'
            assert (project / 'container-test').read_text().strip() == 'container-write'
            assert not (project / '.sandbox/host.sock').exists()
            run('''
                hostctl status > rpc-restarted.json
                python3 -c 'import json; old=json.load(open("rpc-status.json")); new=json.load(open("rpc-restarted.json")); assert new["network"] is False and new["state"] == "running" and new["wrapper_pid"] != old["wrapper_pid"]'
                test "$(cat ~/persistent-test)" = persisted
                test "$(git config --global user.name)" = guest-review-user
                test "$(git config --global user.email)" = guest-review@example.invalid
                grep -q 'region = eu-west-1' ~/.aws/config
                grep -q 'aws_access_key_id = guest-key' ~/.aws/credentials
                test "$(cat ~/.kube/config)" = guest-kube-config
                docker run --rm wrapper-built grep built-inside-vm /build-test
                docker run --rm -v wrapper-test:/data alpine:3.20 grep docker-persisted /data/test
                ! touch .git/forbidden
                ! curl --fail --max-time 3 https://example.com
            ''', '--no-net', expected_update=True)
            run('touch .git/allowed; exit 23', '--git', expected=23)
            assert (project / '.git/allowed').exists()
            run('! touch .git/forbidden')
            run('''
                aws --version
                gh --version
                test "$AWS_ACCESS_KEY_ID" = test-aws-key
                test "$AWS_SECRET_ACCESS_KEY" = test-aws-secret
                test "$AWS_SESSION_TOKEN" = test-aws-session
                test "$AWS_DEFAULT_REGION" = eu-north-1
                test "$GH_TOKEN" = test-gh-token
                test "$WRAPPER_TEST_SECRET" = test-secret-value
                test "$(aws configure get region)" = eu-west-1
                grep -q 'aws_access_key_id = guest-key' ~/.aws/credentials
                ! grep -q test-aws-secret ~/.aws/credentials
                read -r line; test "$line" = stdin-test
                ! read -r extra
            ''', '--aws', 'test-profile', '--gh', '--pass-env', 'WRAPPER_TEST_SECRET', input='stdin-test\n')
            for path in (project / '.sandbox').rglob('*.db'):
                assert b'test-aws-secret' not in path.read_bytes()
                assert b'test-gh-token' not in path.read_bytes()

            with socket.socket() as probe:
                probe.bind(('127.0.0.1', 0))
                port = probe.getsockname()[1]
            server = subprocess.Popen([*command, '--port', f'{port}:8080', '--',
                                       'python3', '-m', 'http.server', '8080', '--bind', '0.0.0.0'],
                                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                deadline = time.monotonic() + 120
                while True:
                    try:
                        with urllib.request.urlopen(f'http://127.0.0.1:{port}/workspace-test', timeout=2) as response:
                            assert response.read().strip() == b'shared'
                        break
                    except OSError:
                        if server.poll() is not None or time.monotonic() > deadline:
                            raise
                        time.sleep(0.2)
            finally:
                server.terminate()
                assert server.wait(timeout=120) == 128 + signal.SIGTERM

            check_terminal(command, 'resize')
            check_terminal(command, 'interrupt')

            check_worktrees(base, codex, project)
            print('Live VM scenarios passed', flush=True)
        finally:
            subprocess.run([*command, '--reset'], timeout=120, check=True)


if __name__ == '__main__':
    main()
