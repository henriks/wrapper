"""Concurrent real VM exec sessions, including an attached interactive shell."""
import json
import os
from pathlib import Path
import pty
import select
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def wait_for(predicate, timeout=120):
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise TimeoutError('shared session did not become ready')
        time.sleep(0.05)


def main():
    with tempfile.TemporaryDirectory(prefix='wrapper-shared-') as directory:
        project = Path(directory) / 'project'
        project.mkdir()
        codex = Path(directory) / 'codex'
        codex.mkdir()
        subprocess.run(['git', 'init', '-q', str(project)], check=True)
        command = [str(ROOT / 'codex-wrap'), '--project', str(project), '--codex-home', str(codex)]
        first = subprocess.Popen([*command, '--git', '--', 'bash', '-ec', '''
            docker pull alpine:3.20
            docker volume create shared-session
            hostctl status > first-status.json
            echo $$ > first-guest.pid
            echo ready > first-ready
            while :; do sleep 1; done
        '''])
        terminal = None
        master = None
        try:
            wait_for(lambda: (project / 'first-ready').exists())
            pid_file, = (project / '.sandbox').rglob('agent.pid')
            vm_pid = pid_file.read_text()
            owner = json.loads((project / 'first-status.json').read_text())['wrapper_pid']
            # No repeated machine flags: --entrypoint bash inherits the active VM,
            # including its writable git metadata and disposable Codex share.
            master, slave = pty.openpty()
            terminal = subprocess.Popen([str(ROOT / 'codex-wrap'), '--project', str(project), '--entrypoint', 'bash'],
                                        stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
            os.close(slave)
            # Wait for bash before sending input: the VM's PTY setup may flush
            # input queued before the interactive exec establishes raw mode.
            output = bytearray()
            def prompt_ready():
                if select.select([master], [], [], 0)[0]:
                    output.extend(os.read(master, 65536))
                assert terminal.poll() is None, output.decode(errors='replace')
                return b'$ ' in output
            wait_for(prompt_ready)
            os.write(master, b'test -t 0 && test -t 1 && docker volume inspect shared-session >/dev/null && touch .git/shared-terminal && hostctl status > terminal-status.json && echo ready > terminal-ready\n')
            def terminal_ready():
                if select.select([master], [], [], 0)[0]:
                    output.extend(os.read(master, 65536))
                assert terminal.poll() is None, output.decode(errors='replace')
                return (project / 'terminal-ready').exists()
            wait_for(terminal_ready)
            assert json.loads((project / 'terminal-status.json').read_text())['wrapper_pid'] == owner
            assert pid_file.read_text() == vm_pid
            assert (project / '.git/shared-terminal').exists()
            for flags in (['--no-net'], ['--reset']):
                rejected = subprocess.run([*command, *flags, '--', 'true'], capture_output=True, timeout=15)
                assert rejected.returncode != 0, rejected
                assert terminal.poll() is None
                assert pid_file.read_text() == vm_pid
            # Ending the initial client must leave its supervisor and the
            # attached terminal alive, including the guest's hostctl endpoint.
            first.terminate()
            assert first.wait(timeout=30) == 128 + signal.SIGTERM
            os.write(master, b'hostctl status > after-exit.json; if kill -0 "$(cat first-guest.pid)" 2>/dev/null; then touch abandoned-process; fi; echo persisted > ~/shared-persist; echo ready > after-exit-ready\n')
            wait_for(lambda: (project / 'after-exit-ready').exists())
            assert json.loads((project / 'after-exit.json').read_text())['wrapper_pid'] == owner
            assert not (project / 'abandoned-process').exists(), 'terminated client left its guest command running'
            assert pid_file.read_text() == vm_pid
            os.write(master, b'exit\n')
            assert terminal.wait(timeout=60) == 0, output.decode(errors='replace')
            assert not (project / '.sandbox/host.sock').exists()
            assert not pid_file.exists()
            subprocess.run([*command, '--', 'bash', '-ec', 'test "$(cat ~/shared-persist)" = persisted; docker volume inspect shared-session >/dev/null'], check=True, timeout=120)
            print('Shared VM, attached terminal, configuration guards and last-session shutdown passed', flush=True)
        finally:
            for process in (first, terminal):
                if process is not None and process.poll() is None:
                    process.terminate()
                    process.wait(timeout=60)
            if master is not None:
                os.close(master)
            subprocess.run([*command, '--reset'], check=True, timeout=60)


if __name__ == '__main__':
    main()
