"""Project VM ownership and session leases; no guest control RPC is exposed."""
from contextlib import contextmanager
import fcntl
import os
import signal
import sys
import time
import traceback


def exclusive(file):
    try:
        fcntl.flock(file, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return True
    except BlockingIOError:
        return False


def supervise(gate, owner, lease, ready, state, boot):
    """Own the VM/RPC independently of any foreground client."""
    lease.close()  # Only the foreground parent holds this session's lease.
    code = 1
    try:
        os.setsid()
        (state / 'supervisor.error').unlink(missing_ok=True)
        with open(os.devnull) as stdin, (state / 'supervisor.log').open('w') as log:
            os.dup2(stdin.fileno(), 0)
            os.dup2(log.fileno(), 1)
            os.dup2(log.fileno(), 2)
        def terminate(signum, frame):
            raise SystemExit(128 + signum)
        for sig in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
            signal.signal(sig, terminate)
        with boot():
            try:
                sys.stdout.flush()
                sys.stderr.flush()
                os.write(ready, b'1')
                os.close(ready)
                ready = None
                fcntl.flock(gate, fcntl.LOCK_UN)
                with (state / 'sessions.lock').open('a') as sessions:
                    while True:
                        fcntl.flock(gate, fcntl.LOCK_EX)
                        if exclusive(sessions):
                            break  # No clients; keep the lifecycle gate through stop.
                        fcntl.flock(gate, fcntl.LOCK_UN)
                        time.sleep(0.1)
            finally:
                fcntl.flock(gate, fcntl.LOCK_EX)
        code = 0
    except BaseException:
        (state / 'supervisor.error').touch()
        traceback.print_exc()
    finally:
        if ready is not None:
            os.close(ready)
        sys.stdout.flush()
        sys.stderr.flush()
        owner.close()
        gate.close()
        os._exit(code)


@contextmanager
def project_session(project, boot, attach, reset=False):
    """Serialize admission, share a live VM, and wait for last-session cleanup.

    Kernel flock leases disappear even when a client is killed. The owner lock
    identifies a live supervisor; the project gate orders admission and shutdown.
    """
    state = project / '.sandbox'
    lease = None
    reader = None
    pid = None
    try:
        while True:
            if (project / '.sandbox-wrap.lock').is_symlink():
                raise ValueError('project lifecycle lock must not be a symlink')
            with (project / '.sandbox-wrap.lock').open('a') as gate:
                fcntl.flock(gate, fcntl.LOCK_EX)
                if state.is_symlink():
                    raise ValueError('.sandbox must not be a symlink')
                internal = {'config.json', 'exports', 'export-mask', 'owner.lock', 'sessions.lock', 'supervisor.log', 'supervisor.error'}
                if (not reset and state.exists() and not (state / 'launch.json').exists()
                        and any(p.name not in internal for p in state.iterdir())):
                    raise ValueError('unrecognized .sandbox state; move it aside or use --reset')
                state.mkdir(mode=0o700, exist_ok=True)
                with (state / 'owner.lock').open('a') as owner:
                    lease = (state / 'sessions.lock').open('a')
                    owns = exclusive(owner)
                    active = not exclusive(lease)
                    if not owns and not active:
                        # Last client has left. Let the old owner stop before a
                        # new generation starts or changes its configuration.
                        lease.close()
                        lease = None
                        gate.close()
                        fcntl.flock(owner, fcntl.LOCK_EX)
                        continue
                    if reset:
                        if active:
                            raise ValueError('cannot reset while project sessions are active')
                        lease.close()
                        lease = None
                        yield
                        return
                    if owns and active:
                        raise ValueError('VM supervisor exited; close remaining sessions before restarting')
                    if not owns:
                        attach()
                        fcntl.flock(lease, fcntl.LOCK_SH)
                    else:
                        fcntl.flock(lease, fcntl.LOCK_SH)
                        reader, writer = os.pipe()
                        sys.stdout.flush()
                        sys.stderr.flush()
                        pid = os.fork()
                        if pid == 0:
                            os.close(reader)
                            supervise(gate, owner, lease, writer, state, boot)
                        os.close(writer)
            break
        if reader is not None:
            if os.read(reader, 1) != b'1':
                raise RuntimeError(f'VM startup failed; see {state / "supervisor.log"}')
            sys.stdout.write((state / 'supervisor.log').read_text())
            sys.stdout.flush()
        yield
    finally:
        if reader is not None:
            os.close(reader)
        if lease is not None:
            with (project / '.sandbox-wrap.lock').open('a') as gate:
                fcntl.flock(gate, fcntl.LOCK_EX)
                lease.close()
                with (state / 'sessions.lock').open('a') as sessions:
                    last = exclusive(sessions)
            if last:
                with (state / 'owner.lock').open('a') as owner:
                    fcntl.flock(owner, fcntl.LOCK_EX)  # Stop/RPC cleanup completes first.
            if pid is not None:
                os.waitpid(pid, 0 if last else os.WNOHANG)
            if last and (state / 'supervisor.error').exists():
                raise RuntimeError(f'VM supervisor failed; see {state / "supervisor.log"}')
