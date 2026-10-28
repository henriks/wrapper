import json
import os
from pathlib import Path
import random
import string
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import host_mounts as h


class NamespaceTests(unittest.TestCase):
    def test_reexec_preserves_uid_and_records_original_namespace(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(h.os, 'readlink', return_value='mnt:[1]'), \
             patch.object(h.HELPER.__class__, 'exists', return_value=False), \
             patch.object(h.shutil, 'which', return_value='/usr/bin/unshare'), \
             patch.object(h.os, 'execvpe') as execute:
            h.ensure_namespace()
        helper, args, env = execute.call_args.args
        self.assertEqual(helper, '/usr/bin/unshare')
        self.assertIn('--map-current-user', args)
        self.assertIn('--keep-caps', args)
        self.assertNotIn('--map-root-user', args)
        self.assertEqual(env[h.NAMESPACE], 'mnt:[1]')

    def test_failed_namespace_transition_is_not_accepted(self):
        with patch.dict(os.environ, {h.NAMESPACE: 'mnt:[1]'}), \
             patch.object(h.os, 'readlink', return_value='mnt:[1]'):
            with self.assertRaisesRegex(RuntimeError, 'not created'):
                h.ensure_namespace()


class ViewTests(unittest.TestCase):
    def prepare(self, root, mounts, *, git=False, table=None):
        project = root / 'project'
        project.mkdir(exist_ok=True)
        (project / '.sandbox').mkdir(exist_ok=True)
        (project / '.sandbox-wrap.lock').touch()
        calls = []
        table = table or [{'target': '/', 'fsroot': '/', 'maj:min': '1:1'}]
        table = [dict(entry, id=index + 1) for index, entry in enumerate(table)]
        visible = {}
        def mount_id(path):
            path = Path(path)
            if str(path) in visible:
                return visible[str(path)]['id']
            return max((entry for entry in table if path.is_relative_to(entry['target'])),
                       key=lambda entry: len(Path(entry['target']).parts))['id']
        def snapshot(args, **kwargs):
            if args[-1] == 'TARGET,FSROOT,MAJ:MIN,ID':
                return json.dumps({'filesystems': table})
            mounted = {}
            for call in calls:
                target = call[-1]
                options = call[call.index('-o') + 1].split(',') if '-o' in call else []
                flags = 'ro' if 'ro' in options or 'ro=recursive' in options else 'rw'
                if any(flags2 == 'ro' and Path(target).is_relative_to(path) for path, flags2 in mounted.items()):
                    flags = 'ro'
                mounted[target] = flags
            visible.update({path: {'target': path, 'vfs-options': flags, 'id': index + 100}
                            for index, (path, flags) in enumerate(mounted.items())})
            return json.dumps({'filesystems': list(visible.values())})
        with patch.object(h, '_mount_id', side_effect=mount_id), \
             patch.object(h.subprocess, 'check_output', side_effect=snapshot), \
             patch.object(h.subprocess, 'run', side_effect=lambda args, **kwargs: calls.append(list(args))):
            exported = h.prepare(project, mounts, git)
        return exported, calls

    def test_masks_control_paths_and_git_in_the_writable_workspace(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); project = root / 'project'; project.mkdir(); (project / '.git').mkdir()
            exported, calls = self.prepare(root, [f'{project}:{project}:rw'])
            base = project / '.sandbox/exports'
            view = base / '0'
            self.assertEqual(exported, [f'{view}:{project}:rw'])
            self.assertIn(['mount', '--make-unbindable', str(base)], calls)
            self.assertIn(['mount', '-t', 'tmpfs', '-o', 'size=4k,mode=000,ro', 'tmpfs', str(view / '.sandbox')], calls)
            self.assertIn(['mount', '-o', 'bind,ro', str(project / '.sandbox/export-mask'), str(view / '.sandbox-wrap.lock')], calls)
            self.assertIn(['mount', '-o', 'rbind,ro=recursive', str(view / '.git'), str(view / '.git')], calls)
            self.assertFalse(any(args[0] == 'umount' for args in calls))

    def test_readonly_views_are_recursive_and_ancestors_are_anchored(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); project = root / 'project'; project.mkdir()
            data = root / 'data'; protected = data / 'nested/private'; protected.mkdir(parents=True)
            _, calls = self.prepare(root, [f'{project}:{project}:rw', f'{data}:/data:rw', f'{protected}:/private:ro'])
            base = project / '.sandbox/exports'
            self.assertIn(['mount', '--rbind', str(base / '1/nested'), str(base / '1/nested')], calls)
            self.assertIn(['mount', '-o', 'rbind,ro=recursive', str(base / '1/nested/private'), str(base / '1/nested/private')], calls)
            self.assertIn(['mount', '-o', 'rbind,ro=recursive', str(protected), str(base / '2')], calls)

    def test_alias_exports_get_the_same_masks_and_cannot_expose_control_state(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); project = root / 'project'; project.mkdir()
            alias = root / 'alias'; alias.mkdir()
            table = [{'target': str(alias), 'fsroot': str(project), 'maj:min': '1:1'},
                     {'target': '/', 'fsroot': '/', 'maj:min': '1:1'}]
            _, calls = self.prepare(root, [f'{project}:{project}:rw', f'{alias}:/alias:rw'], table=table)
            target = project / '.sandbox/exports/1/.sandbox'
            self.assertTrue(any(args[-1] == str(target) and 'tmpfs' in args for args in calls))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); project = root / 'project'; project.mkdir(); alias = root / 'alias'; alias.mkdir()
            table = [{'target': str(alias), 'fsroot': str(project / '.sandbox/vms'), 'maj:min': '1:1'},
                     {'target': '/', 'fsroot': '/', 'maj:min': '1:1'}]
            with self.assertRaisesRegex(ValueError, 'control state'):
                self.prepare(root, [f'{project}:{project}:rw', f'{alias}:/alias:rw'], table=table)

    def test_aliases_nested_inside_a_parent_export_are_masked(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); project = root / 'project'; project.mkdir(); (project / '.git').mkdir()
            alias = root / 'alias'; alias.mkdir()
            table = [{'target': str(alias), 'fsroot': str(project / '.sandbox'), 'maj:min': '1:1'},
                     {'target': '/', 'fsroot': '/', 'maj:min': '1:1'}]
            _, calls = self.prepare(root, [f'{project}:{project}:rw', f'{root}:/parent:rw'], table=table)
            base = project / '.sandbox/exports/1'
            self.assertIn(['mount', '-t', 'tmpfs', '-o', 'size=4k,mode=000,ro', 'tmpfs', str(base / 'alias')], calls)
            self.assertEqual(calls.count(['mount', '--rbind', str(base / 'project'), str(base / 'project')]), 1)

    def test_conflicting_readwrite_export_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); data = root / 'data'; (data / 'child').mkdir(parents=True)
            with self.assertRaisesRegex(ValueError, 'read-only subtree'):
                self.prepare(root, [f'{data}:/ro:ro', f'{data / "child"}:/rw:rw'])

    def test_original_system_paths_are_rejected_before_projection(self):
        for source in ('/', '/etc', '/proc', '/run', '/usr'):
            with tempfile.TemporaryDirectory() as directory:
                with self.assertRaisesRegex(ValueError, 'system path'):
                    self.prepare(Path(directory), [f'{source}:/data:ro'])

    def test_git_write_mode_does_not_make_git_readonly(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); project = root / 'project'; project.mkdir(); (project / '.git').mkdir()
            _, calls = self.prepare(root, [f'{project}:{project}:rw'], git=True)
            self.assertFalse(any(args[-1].endswith('/.git') for args in calls))

    def test_mount_failure_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory); (project / '.sandbox').mkdir(); (project / '.sandbox-wrap.lock').touch()
            table = {'filesystems': [{'target': '/', 'fsroot': '/', 'maj:min': '1:1', 'id': 1}]}
            with patch.object(h, '_mount_id', return_value=1), \
                 patch.object(h.subprocess, 'check_output', return_value=json.dumps(table)), \
                 patch.object(h.subprocess, 'run', side_effect=subprocess.CalledProcessError(1, ['mount'])):
                with self.assertRaisesRegex(RuntimeError, 'host mount protection failed'):
                    h.prepare(project, [f'{project}:{project}:rw'], False)

    def test_missing_kernel_readonly_flags_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory); (project / '.sandbox').mkdir(); (project / '.sandbox-wrap.lock').touch()
            table = {'filesystems': [{'target': '/', 'fsroot': '/', 'maj:min': '1:1', 'id': 1}]}
            actual = {'filesystems': [{'target': str(project / '.sandbox/exports/0/.sandbox'), 'vfs-options': 'rw', 'id': 1}]}
            with patch.object(h, '_mount_id', return_value=1), \
                 patch.object(h.subprocess, 'check_output', side_effect=[json.dumps(table), json.dumps(actual)]), \
                 patch.object(h.subprocess, 'run'):
                with self.assertRaisesRegex(RuntimeError, 'did not enforce'):
                    h.prepare(project, [f'{project}:{project}:rw'], False)

    def test_seeded_path_arguments_are_never_shell_code(self):
        rng = random.Random(774)
        for _ in range(50):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory); project = root / 'project'; project.mkdir()
                name = ''.join(rng.choices(string.ascii_letters + " '$();`", k=30))
                source = root / name; source.mkdir()
                exported, calls = self.prepare(root, [f'{source}:/data:rw'])
                self.assertIn(['mount', '-o', 'rbind', str(source), str(project / '.sandbox/exports/0')], calls)
                self.assertTrue(exported[0].endswith(':/data:rw'))
