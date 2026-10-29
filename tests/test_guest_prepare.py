"""Guest startup rejects missing proxy support before tool downloads."""
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[1]


class GuestPrepareTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='guest-prepare-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        binaries = self.root / 'bin'
        binaries.mkdir()
        # Keep user/group/ownership operations inside this disposable fixture.
        for name in ('getent', 'chown', 'chmod', 'docker'):
            command = binaries / name
            command.write_text('#!/bin/sh\nexit 0\n')
            command.chmod(0o755)
        self.env = dict(os.environ, PATH=f'{binaries}:{os.environ["PATH"]}')
        self.listener = socket.socket()
        self.addCleanup(self.listener.close)
        self.listener.bind(('127.0.0.1', 0))
        source = (ROOT / 'guest/prepare').read_text()
        source = source.replace('/home/agent', str(self.root / 'home'))
        source = source.replace('/run/sandbox-wrap', str(self.root / 'run'))
        source = source.replace('/usr/local/lib/sandbox-wrap/network_proxy.py', str(self.root / 'network_proxy.py'))
        source = source.replace('/4750)', f'/{self.listener.getsockname()[1]})')
        self.script = self.root / 'prepare'
        self.script.write_text(source)

    def prepare(self, proxy):
        return subprocess.run(['bash', str(self.script), str(os.getuid()), str(os.getgid()), str(proxy)],
                              env=self.env, capture_output=True, text=True, timeout=10)

    def test_missing_proxy_reports_image_recreation_guidance(self):
        result = self.prepare(1)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('guest proxy is not listening', result.stderr)
        self.assertIn('--rebuild-image', result.stderr)
        self.assertIn('--reset (deletes guest state)', result.stderr)

    def test_direct_and_off_modes_need_no_proxy(self):
        result = self.prepare(0)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_proxy_startup_is_allowed_to_finish(self):
        (self.root / 'network_proxy.py').touch()
        timer = threading.Timer(0.2, self.listener.listen)
        timer.start()
        try:
            result = self.prepare(1)
            self.assertEqual(result.returncode, 0, result.stderr)
        finally:
            timer.cancel()
            timer.join()
