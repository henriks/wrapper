"""Host-owned filesystem views. Only prepared paths are exported to the VM."""
from functools import cache
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

HELPER = Path('/usr/local/libexec/sandbox-wrap-unshare')
NAMESPACE = '_SANDBOX_WRAP_PARENT_MOUNT_NS'
LAYOUT = 1


def ensure_namespace():
    current = os.readlink('/proc/self/ns/mnt')
    previous = os.environ.get(NAMESPACE)
    if previous is not None:
        if previous == current:
            raise RuntimeError('host mount namespace was not created')
        return
    helper = str(HELPER) if HELPER.exists() else shutil.which('unshare')
    if helper is None:
        raise RuntimeError('host mount protection requires util-linux unshare')
    env = dict(os.environ, **{NAMESPACE: current})
    os.execvpe(helper, [helper, '--user', '--map-current-user', '--mount',
                       '--propagation', 'private', '--keep-caps', sys.executable,
                       str(Path(sys.argv[0]).resolve()), *sys.argv[1:]], env)


def volumes(state, mounts):
    """Stable exported paths; logical sources remain in the launch record."""
    return [f'{state / "exports" / str(index)}:{item.split(":")[1]}:{item.split(":")[2]}'
            for index, item in enumerate(mounts)]


def _mount_id(path):
    descriptor = os.open(path, os.O_PATH)
    try:
        info = Path(f'/proc/self/fdinfo/{descriptor}').read_text().splitlines()
        return int(next(line.split()[1] for line in info if line.startswith('mnt_id:')))
    finally:
        os.close(descriptor)


def prepare(project, mounts, git_writable):
    """Compose all views before starting a VMM or host service threads.

    Mounts live until the namespace's last process exits. Explicit unmounting
    during error cleanup could expose a running VMM to the underlying files.
    """
    def run(*args):
        subprocess.run(args, check=True, timeout=30)

    table = json.loads(subprocess.check_output(
        ['findmnt', '--json', '--list', '--output', 'TARGET,FSROOT,MAJ:MIN,ID'],
        text=True, timeout=10))['filesystems']
    by_id = {entry['id']: entry for entry in table}

    @cache
    def backing(path):
        entry = by_id[_mount_id(path)]
        return entry, Path(entry['fsroot']) / path.relative_to(entry['target'])

    def location(path):
        entry, physical = backing(path)
        return entry['maj:min'], physical

    def routes(path, source):
        found = set()
        direct = relative(path, source)
        if direct is not None:
            found.add(direct)
        device, physical = location(path)
        for entry in table:
            target = Path(entry['target'])
            if not target.is_relative_to(source):
                continue
            try:
                if backing(target)[0]['id'] != entry['id']:
                    continue  # Covered mounts are not part of the exported view.
            except FileNotFoundError:
                continue
            if entry['maj:min'] == device and physical.is_relative_to(entry['fsroot']):
                found.add(target.relative_to(source) / physical.relative_to(entry['fsroot']))
        return found

    def relative(child, parent):
        if child.is_relative_to(parent):
            return child.relative_to(parent)
        child_device, child_path = location(child)
        parent_device, parent_path = location(parent)
        if child_device == parent_device and child_path.is_relative_to(parent_path):
            return child_path.relative_to(parent_path)
        return None

    state = project / '.sandbox'
    gate = project / '.sandbox-wrap.lock'
    sources = [(Path(source), mode == 'ro') for source, _, mode in (item.split(':') for item in mounts)]
    hidden = [state, gate]
    readonly = [source for source, ro in sources if ro]
    git = project / '.git'
    if not git_writable and git.exists():
        if git.is_symlink():
            raise ValueError('read-only Git protection requires a .git directory or worktree file, not a symlink')
        readonly.append(git)
    # Include nested filesystem roots, so a separately shared bind alias cannot
    # circumvent a parent subtree's policy across filesystem boundaries.
    for rules in (hidden, readonly):
        for entry in table:
            target = Path(entry['target'])
            if not any(target.is_relative_to(path) for path in list(rules)):
                continue
            try:
                if backing(target)[0]['id'] == entry['id']:
                    rules.append(target)
            except FileNotFoundError:
                pass

    system = [Path(path).resolve() for path in ('/etc', '/boot', '/usr', '/bin', '/sbin', '/lib', '/proc', '/sys', '/dev', '/run', '/var/log')]
    for source, ro in sources:
        # smolvm sees staging paths, so retain its original-source safety checks.
        if (any(relative(source, Path(path)) == Path('.') for path in ('/', '/var'))
                or any(relative(source, path) is not None for path in system)):
            raise ValueError(f'cannot share a protected host system path: {source}')
        if any(relative(source, path) is not None for path in hidden):
            raise ValueError(f'cannot share host control state: {source}')
        if not ro and any(relative(source, path) is not None for path in readonly):
            raise ValueError(f'writable share overlaps a read-only subtree: {source}')

    base = state / 'exports'
    if base.is_symlink():
        raise ValueError('host export directory must not be a symlink')
    base.mkdir(exist_ok=True)
    try:
        run('mount', '-t', 'tmpfs', '-o', 'size=1m,mode=700', 'tmpfs', str(base))
        # Recursive source binds prune this mount, avoiding recursive copies
        # when an exported source contains .sandbox/exports itself.
        run('mount', '--make-unbindable', str(base))
        # An unbindable mount cannot itself supply a bind-mount source.
        blank = state / 'export-mask'
        descriptor = os.open(blank, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
        os.close(descriptor)
        readonly_views = []
        for index, (source, ro) in enumerate(sources):
            view = base / str(index)
            view.mkdir()
            run('mount', '-o', 'rbind,ro=recursive' if ro else 'rbind', str(source), str(view))
            if ro:
                readonly_views.append(view)
            rules = [(path, 'ro') for path in readonly if not ro]
            rules += [(path, 'hide') for path in hidden]
            targets = {(suffix, path, policy) for path, policy in rules for suffix in routes(path, source)}
            anchored = set()
            masked = set()
            for suffix, path, policy in sorted(targets, key=lambda item: (len(item[0].parts), str(item[0]), item[2])):
                if suffix == Path('.') or any(suffix.is_relative_to(parent) for parent in masked):
                    continue
                # Recursive anchors retain child mounts and keep covered files hidden.
                # Anchor once so later policies cannot replace earlier masks.
                for parent in reversed(suffix.parents):
                    target = view / parent
                    if parent != Path('.') and target not in anchored:
                        run('mount', '--rbind', str(target), str(target))
                        anchored.add(target)
                target = view / suffix
                readonly_views.append(target)
                if policy == 'hide' and path.is_dir():
                    run('mount', '-t', 'tmpfs', '-o', 'size=4k,mode=000,ro', 'tmpfs', str(target))
                    masked.add(suffix)
                elif policy == 'hide':
                    run('mount', '-o', 'bind,ro', str(blank), str(target))
                else:
                    run('mount', '-o', 'rbind,ro=recursive' if path.is_dir() else 'bind,ro', str(target), str(target))
                anchored.add(target)
        # Inspect kernel mount flags instead of trusting mount(8) to understand
        # recursive attributes on every supported host. Failure stays closed.
        if readonly_views:
            actual = json.loads(subprocess.check_output(
                ['findmnt', '--json', '--list', '--output', 'TARGET,VFS-OPTIONS,ID'],
                text=True, timeout=10))['filesystems']
            visible = {}
            for entry in actual:
                target = Path(entry['target'])
                if not any(target.is_relative_to(view) for view in readonly_views):
                    continue
                try:
                    if _mount_id(target) == entry['id']:
                        visible[target] = entry['vfs-options'].split(',')
                except FileNotFoundError:
                    pass  # A covered subtree may no longer exist in the view.
            for view in readonly_views:
                if view not in visible or any('ro' not in flags for path, flags in visible.items() if path.is_relative_to(view)):
                    raise RuntimeError(f'host did not enforce recursive read-only mounts: {view}')
    except subprocess.CalledProcessError as error:
        raise RuntimeError('host mount protection failed; run the checkout\'s ./setup for AppArmor support; '
                           'requires user namespaces, Linux 5.12+ and util-linux 2.39+') from error
    return volumes(state, mounts)
