"""Real process/lock tests with a tiny fake VM lifecycle."""
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
CLIENT = '''
from contextlib import contextmanager
from pathlib import Path
import sys
from vm_session import project_session
project = Path(sys.argv[1])
@contextmanager
def boot():
    if 'fail-boot' in sys.argv: raise RuntimeError('injected boot failure')
    if 'fail-migration' in sys.argv: raise SystemExit('error: this VM has no proxy bridge; rebuild the image and --reset to recreate it')
    with (project / 'events').open('a') as out: out.write('boot\\n')
    (project / '.sandbox/launch.json').write_text('{}')
    try: yield
    finally:
        with (project / 'events').open('a') as out: out.write('stop\\n')
        if 'fail-stop' in sys.argv: raise RuntimeError('injected stop failure')
def attach():
    with (project / 'events').open('a') as out: out.write('attach\\n')
with project_session(project, boot, attach, reset='reset' in sys.argv):
    print('ready', flush=True)
    sys.stdin.readline()
'''


class SessionTests(unittest.TestCase):
    def test_failed_startup_can_be_retried(self):
        with tempfile.TemporaryDirectory() as directory:
            for flag, message in (('fail-boot', b'injected boot failure'),
                                  ('fail-migration', b'this VM has no proxy bridge; rebuild the image and --reset')):
                with self.subTest(flag=flag):
                    failed = subprocess.run([sys.executable, '-c', CLIENT, directory, flag], cwd=ROOT,
                                            input=b'\n', capture_output=True, timeout=10)
                    self.assertNotEqual(failed.returncode, 0)
                    self.assertIn(message, failed.stderr)
            retried = subprocess.run([sys.executable, '-c', CLIENT, directory], cwd=ROOT,
                                     input=b'\n', capture_output=True, timeout=10)
            self.assertEqual(retried.returncode, 0, retried.stderr)

    def test_shutdown_failure_is_reported(self):
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run([sys.executable, '-c', CLIENT, directory, 'fail-stop'], cwd=ROOT,
                                    input=b'\n', capture_output=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b'VM supervisor failed', result.stderr)
            self.assertIn(b'injected stop failure', result.stderr)


    def test_owner_outlives_first_client_and_last_client_stops(self):
        for killed in (False, True):
            with self.subTest(killed=killed), tempfile.TemporaryDirectory() as directory:
                project = Path(directory)
                clients = []
                try:
                    for _ in range(2):
                        process = subprocess.Popen([sys.executable, '-c', CLIENT, directory], cwd=ROOT,
                                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                        clients.append(process)
                        self.assertTrue(select.select([process.stdout], [], [], 10)[0], 'client did not attach')
                        self.assertEqual(process.stdout.readline(), b'ready\n')
                    reset = subprocess.run([sys.executable, '-c', CLIENT, directory, 'reset'], cwd=ROOT,
                                           input=b'', capture_output=True, timeout=10)
                    self.assertNotEqual(reset.returncode, 0)
                    self.assertIn(b'active', reset.stderr)
                    if killed:
                        clients[0].kill()
                    else:
                        clients[0].stdin.write(b'\n')
                        clients[0].stdin.flush()
                    clients[0].wait(timeout=10)
                    self.assertIsNone(clients[1].poll())
                    self.assertEqual((project / 'events').read_text().splitlines(), ['boot', 'attach'])
                    clients[1].stdin.write(b'\n')
                    clients[1].stdin.flush()
                    self.assertEqual(clients[1].wait(timeout=10), 0)
                    self.assertEqual((project / 'events').read_text().splitlines(), ['boot', 'attach', 'stop'])
                finally:
                    for process in clients:
                        if process.poll() is None:
                            process.kill()
                        process.communicate(timeout=10)
