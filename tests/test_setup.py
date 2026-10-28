import importlib.machinery
import importlib.util
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_loader('wrapper_setup', importlib.machinery.SourceFileLoader(
    'wrapper_setup', str(Path(__file__).resolve().parents[1] / 'setup')))
s = importlib.util.module_from_spec(spec)
spec.loader.exec_module(s)


class SetupTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.bin = self.root / 'bin'
        for name in ('APPARMOR', 'RESTRICTION', 'HELPER', 'PROFILE'):
            patcher = patch.object(s, name, self.root / name.lower())
            patcher.start()
            self.addCleanup(patcher.stop)

    def restrict(self):
        s.APPARMOR.write_text('Y\n')
        s.RESTRICTION.write_text('1\n')

    def test_unrestricted_install_is_idempotent_and_link_works_outside_checkout(self):
        with patch.object(s.subprocess, 'run') as run:
            s.setup(self.bin)
            s.setup(self.bin)
        run.assert_not_called()
        self.assertEqual(list(self.bin.iterdir()), [self.bin / 'codex-wrap'])
        link = self.bin / 'codex-wrap'
        self.assertEqual(link.resolve(), s.ROOT / 'sandbox-wrap')
        result = subprocess.run([str(link), '--help'], cwd=self.root, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('--entrypoint', result.stdout)

    def test_disabled_apparmor_needs_no_system_changes(self):
        self.restrict()
        s.APPARMOR.write_text('N\n')
        with patch.object(s.subprocess, 'run') as run:
            s.setup(self.bin)
        run.assert_not_called()

    def test_restricted_install_copies_helper_and_loads_profile(self):
        self.restrict()
        parser = self.root / 'apparmor_parser'
        parser.write_text('#!/bin/sh\ntest "$1" = -r && test -r "$2"\n')
        parser.chmod(0o755)
        which = shutil.which
        with patch.object(s.os, 'geteuid', return_value=0), patch.object(s.shutil, 'which',
                side_effect=lambda name, **kw: str(parser) if name == 'apparmor_parser' else which(name, **kw)):
            s.setup(self.bin)
        self.assertFalse(s.HELPER.is_symlink())
        self.assertEqual(s.HELPER.read_bytes(), Path(which('unshare')).read_bytes())
        self.assertEqual(s.HELPER.stat().st_mode & 0o777, 0o755)
        self.assertEqual(s.PROFILE.read_bytes(), (s.ROOT / 'host/sandbox-wrap-unshare.apparmor').read_bytes())

    def test_user_install_elevates_only_system_commands(self):
        self.restrict()
        with patch.object(s.os, 'geteuid', return_value=1000), \
             patch.object(s.shutil, 'which', side_effect=lambda name, **kw: f'/tools/{name}'), \
             patch.object(s.subprocess, 'run') as run:
            s.setup(self.bin)
        self.assertTrue((self.bin / 'codex-wrap').is_symlink())
        self.assertEqual(len(run.call_args_list), 3)
        for call in run.call_args_list:
            self.assertEqual(call.args[0][0], '/tools/sudo')

    def test_missing_apparmor_tools_fail_before_installing(self):
        self.restrict()
        with patch.object(s.shutil, 'which', return_value=None), \
             patch.object(s.subprocess, 'run') as run:
            with self.assertRaisesRegex(RuntimeError, 'install util-linux and the AppArmor tools'):
                s.setup(self.bin)
        run.assert_not_called()
        self.assertFalse(self.bin.exists())

    def test_existing_file_is_preserved_before_system_changes(self):
        self.bin.mkdir()
        existing = self.bin / 'codex-wrap'
        existing.write_text('keep')
        with patch.object(s.subprocess, 'run') as run:
            with self.assertRaisesRegex(RuntimeError, 'refusing to replace'):
                s.setup(self.bin)
        run.assert_not_called()
        self.assertEqual(existing.read_text(), 'keep')
        self.assertFalse((self.bin / 'sandbox-wrap').exists())
