# ComposedFs Filesystem Semantics Baseline

Ticket: `wra-30di`

Date: 2026-05-12

## Outcome

`ComposedFs` v1 must be a real writable development filesystem for the mounted
host-backed subtrees, not only a lookup/read shim. The implementation can defer
rare FUSE operations, but it must correctly support normal shell, agent,
package-manager, Git, and Docker bind-mount workflows.

The critical correctness areas are:
- lookup counts and inode lifetime
- safe fd-relative traversal under each mount root
- nested readonly boundaries
- open handles surviving rename/unlink
- enough metadata and xattr behavior for developer tooling
- predictable errors for unsupported cross-mount behavior

This document defines the v1 behavior that backend implementation and tests
should target.

## Workloads That Must Work

The baseline is driven by these wrapper workflows:

- guest init mounts the composed export and bind-mounts workspace, tool state,
  auth/config, and user-requested paths
- guest init creates `.sandbox/docker-vm/run/` under the shared workspace and
  writes mirrored logs there
- Codex/Copilot install or run from the host user's natural `$HOME` path in the
  guest, backed by project-local `.sandbox/home` storage where no more specific
  host-backed mount overrides it
- npm, mise, shell startup, and tool caches write under `$HOME/.local`,
  `$HOME/.cache`, `$HOME/.config`, and `$HOME/.local/state`
- Git reads and writes normal workspace files and may use lock-file rename
  patterns
- Docker bind mounts project paths using guest-visible paths that match host
  paths
- `--ro PATH` exposes arbitrary host paths readonly at the same absolute path
- `--rw PATH` exposes arbitrary host paths writable at the same absolute path
- `--gh` exposes GitHub CLI config readonly while auth is passed by token
- Docker client config and tool auth state may be writable tool-state mounts

## Mount Classes

The backend should distinguish these source classes from the manifest:

- `workspace`: writable project tree
- `persistent-home`: project-local backing store mounted at the guest-visible
  host home path
- `tool-state`: writable tool state such as `.codex`, `.copilot`, and
  `.docker` when present
- `auth-config`: readonly host auth/config such as `.config/gh`
- `system-ro`: readonly system data such as certs or `/etc/hosts`
- `user-ro`: user-requested readonly path
- `user-rw`: user-requested writable path

Readonly/writable behavior is based on the manifest entry, not on host path
permissions alone.

## Path Resolution

Required behavior:
- all host-backed traversal is relative to a mount root fd or equivalent safe
  root handle
- guest `..` and symlink traversal must not escape the mount root
- mount roots themselves must be opened without following an attacker-controlled
  final symlink unless the manifest explicitly records that the host resolved
  the source beforehand
- path strings must not be joined to host absolute paths at operation time
- nested mount boundaries must be checked during traversal, not only at lookup
  of the nested root

Preferred implementation:
- use `openat2` with `RESOLVE_BENEATH` / `RESOLVE_IN_ROOT` and no-magic-link
  style protections where available
- provide a documented `openat` fallback if `openat2` is unavailable

## Inode Identity And Lifetime

Upstream `virtiofsd::filesystem::FileSystem` requires lookup-count tracking:

- every returned `Entry` increments lookup count
- `forget` and `batch_forget` decrement lookup counts
- inodes with non-zero lookup count may receive requests after unlink, rmdir,
  or rename
- open files/directories delay final `forget` until `release` / `releasedir`

V1 policy:
- synthetic directories and mount roots have stable inodes for the VM lifetime
- host-backed descendants should prefer identity based on `(mount_id, dev, ino)`
  when host metadata is available
- path aliases to the same host inode inside the same mount should resolve to
  the same backend inode where practical
- if the implementation temporarily falls back to `(mount_id, relative_path)`,
  the limitation must be documented in the backend ticket and tests must mark
  hardlink/open-after-rename behavior as expected failure until fixed
- unlinked or overwritten inodes with open handles or non-zero lookup counts
  must stay addressable through their existing inode/handle until release and
  forget complete

Hardlinks:
- hardlinks within the same writable host directory mount should be supported
  if the host filesystem supports them
- hardlinks across mount ids must fail with `EXDEV`
- hardlinks involving synthetic nodes or file-mount roots may fail with
  `EPERM` or `EOPNOTSUPP`

## Operation Baseline

### Required In V1

These operations are required for v1:

- `init`: advertise only options the backend actually supports
- `lookup`: resolve synthetic children, mount roots, and host-backed children;
  increment lookup counts on returned entries
- `forget` / `batch_forget`: decrement lookup counts and retire stale nodes
  only when safe
- `getattr`: return accurate metadata for synthetic nodes, mount roots, and
  host-backed nodes
- `setattr`: support chmod/chown/truncate/timestamps on writable host-backed
  nodes; reject mutating changes on readonly nodes with `EROFS`
- `access`: enforce access using host metadata and manifest readonly policy;
  do not return `ENOSYS` globally because the kernel treats that as permanent
  success
- `readlink`: return symlink contents for host symlinks; synthetic nodes should
  normally return `EINVAL`
- `symlink`: create symlinks inside writable host-backed directories; reject on
  readonly boundaries
- `mkdir`: create directories in writable host-backed directories
- `create`: create and open regular files in writable host-backed directories
- `mknod`: support regular-file creation if needed as a fallback for `create`;
  special device nodes may fail with `EPERM`
- `unlink`: remove files/symlinks in writable host-backed directories while
  preserving live inode/handle semantics
- `rmdir`: remove directories in writable host-backed directories
- `rename`: support atomic host rename within the same writable mount and
  honor `RENAME_NOREPLACE`; support `RENAME_EXCHANGE` if host support is
  available, otherwise return a clear unsupported error
- `open`: enforce read/write/truncation/append access and return handles for
  host-backed files
- `read`: read exact requested bytes except at EOF or on error
- `write`: write exact requested bytes or return an error; enforce readonly
  policy even if the host fd would be writable
- `flush`: return write/close errors when known; safe no-op is acceptable when
  no delayed errors are tracked
- `fsync`: call host `fsync` / `fdatasync` for writable host-backed files
- `release`: close file handles and retire delete-pending nodes if lookup count
  permits
- `statfs`: return reasonable host-backed filesystem statistics for mounted
  subtrees and stable synthetic defaults for synthetic-only directories
- `opendir`: open synthetic or host-backed directory handles
- `readdir`: enumerate synthetic entries, mount roots, and host-backed entries
  without duplicate or skipped unrelated entries across offsets
- `fsyncdir`: call host directory fsync when available; safe success is
  acceptable if unsupported by host/filesystem
- `releasedir`: close directory handles
- `getxattr` / `listxattr`: delegate for host-backed nodes where supported;
  return `ENODATA` for missing attributes and `EOPNOTSUPP` for unsupported
  namespaces
- `setxattr` / `removexattr`: support writable host-backed nodes where host
  supports it; reject readonly nodes with `EROFS`
- `lseek`: support ordinary `SEEK_SET`, `SEEK_CUR`, `SEEK_END`; support
  `SEEK_DATA` / `SEEK_HOLE` when host supports them or return an appropriate
  unsupported error
- POSIX byte-range locks: advertise `POSIX_LOCKS` when the guest offers it and
  bridge `getlk`, `setlk`, and `setlkw` to Linux OFD locks on host file
  descriptions keyed by guest lock owner. This must coordinate with ordinary
  host POSIX `fcntl` locks for shared SQLite-style workloads.

### Allowed To Defer

These may be deferred in v1 if documented and covered by tests:

- `ioctl`
- `bmap`
- `poll`
- `notify_reply`
- `tmpfile`
- `copyfilerange`
- `syncfs`
- full `fallocate`

If `fallocate` is not implemented, return `ENOSYS` intentionally and document
that the kernel may convert future calls to `EOPNOTSUPP`.

## Readonly Policy

Readonly is a backend-enforced manifest policy. It must hold even when the
underlying host path is writable.

Readonly mounts must reject:
- `create`
- `mknod`
- `mkdir`
- `symlink`
- `unlink`
- `rmdir`
- `rename` into, out of, or within a readonly boundary
- `link` creating a new name inside a readonly boundary
- `open` with write, truncation, append, or creation intent
- `write`
- truncating or mutating `setattr`
- `setxattr`
- `removexattr`
- `fallocate`

Return `EROFS` for readonly-policy failures. Prefer `EXDEV` for cross-mount
renames/links when both sides are otherwise writable.

Nested readonly example:

```text
/home/user            rw
/home/user/.config/gh ro
```

Writes through `/home/user/.config/gh` must fail even if traversal entered via
the writable `/home/user` mount.

## Synthetic Nodes

Synthetic directories exist to connect mount roots, for example `/`,
`/home`, `/home/user`, and `/etc`.

Synthetic directory behavior:
- `lookup` resolves only manifest-derived synthetic children or mount roots
- `readdir` lists direct synthetic children and mount roots
- `getattr` returns directory attributes owned by the guest-visible uid/gid
  policy
- mutation generally fails with `EROFS` or `EPERM`
- synthetic directories are not backed by host paths and must not be used to
  create new arbitrary host paths

If a user wants to create files under a synthetic parent, the target must be
inside a writable mounted subtree.

## File Mounts

File mounts are first-class.

Required behavior:
- parent synthetic directories enumerate file mount names
- `lookup` of the mounted file returns host metadata
- `open`, `read`, `getattr`, `access`, `getxattr`, and `listxattr` work like a
  normal host-backed file
- writable file mounts may support `write`, `setattr`, and xattr mutation if
  manifest marks them `rw`
- file mounts cannot be used as directories
- replacing a file mount path with `rename`, `unlink`, or `create` must fail
  unless the manifest explicitly permits mutating that mounted file

## Cache Policy

Initial policy should prefer correctness:
- no long-lived positive or negative path cache until tests justify it
- attribute TTLs should be short or zero for host-backed nodes because the host
  may modify the same files
- synthetic nodes and mount roots may use longer stable attributes
- open handles should keep fd-backed access stable even if the path is renamed
  or unlinked

## Smoke Commands

These commands should be part of q35 and microvm validation once `ComposedFs`
is integrated.

Shell/path basics:

```sh
pwd
ls -la "$PWD"
mkdir -p .sandbox/docker-vm/run
printf test > .sandbox/docker-vm/run/fs-smoke.txt
cat .sandbox/docker-vm/run/fs-smoke.txt
mv .sandbox/docker-vm/run/fs-smoke.txt .sandbox/docker-vm/run/fs-smoke.renamed
rm .sandbox/docker-vm/run/fs-smoke.renamed
```

Open-after-rename/unlink:

```sh
python3 - <<'PY'
from pathlib import Path
p = Path(".sandbox/docker-vm/run/open-handle.txt")
p.write_text("before")
f = p.open("r")
p.rename(p.with_suffix(".renamed"))
assert f.read() == "before"
f.close()
p.with_suffix(".renamed").unlink()
PY
```

Readonly:

```sh
test -r /path/from/--ro
! sh -c 'echo fail >> /path/from/--ro/probe' 2>/tmp/ro-error
```

Git:

```sh
git status --short
git rev-parse --show-toplevel
git diff --stat
```

Package/tool state:

```sh
mkdir -p "$HOME/.cache" "$HOME/.local/bin" "$HOME/.config"
printf '{}' > "$HOME/.config/fs-smoke.json"
npm config get prefix
```

SQLite-style state, where Python's `sqlite3` module is present:

```sh
python3 - <<'PY'
import os, sqlite3
root = os.path.join(os.environ["HOME"], ".cache", "agentvm-sqlite-smoke")
os.makedirs(root, exist_ok=True)
db = os.path.join(root, "state.sqlite")
conn = sqlite3.connect(db, timeout=1.0)
assert conn.execute("PRAGMA journal_mode=WAL").fetchone()[0].lower() == "wal"
conn.execute("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)")
with conn:
    conn.execute("INSERT INTO kv VALUES('key', 'value') ON CONFLICT(k) DO UPDATE SET v=excluded.v")
conn.close()
conn = sqlite3.connect(db, timeout=1.0)
assert conn.execute("SELECT v FROM kv WHERE k='key'").fetchone()[0] == "value"
conn.close()
PY
```

Docker bind mount:

```sh
printf docker-bind > docker-bind-smoke.txt
docker run --rm -v "$PWD:/work" alpine:3.22 cat /work/docker-bind-smoke.txt
rm docker-bind-smoke.txt
```

Codex/Copilot state visibility:

```sh
test -d "$HOME/.codex" || true
test -d "$HOME/.copilot" || true
test -d "$HOME/.config/github-copilot" || true
```

GitHub config when `--gh` is used:

```sh
test -d "$HOME/.config/gh" || true
! sh -c 'echo fail > "$HOME/.config/gh/fs-smoke"' 2>/tmp/gh-ro-error
```

Xattrs, where host filesystem supports them:

```sh
touch .sandbox/docker-vm/run/xattr-smoke
setfattr -n user.agentvm -v ok .sandbox/docker-vm/run/xattr-smoke
getfattr -n user.agentvm .sandbox/docker-vm/run/xattr-smoke
```

## Test Coverage Required By This Baseline

Backend correctness tests should cover:
- synthetic lookup/readdir
- file mounts and directory mounts
- nested ro/rw boundaries
- symlink escape attempts and `..` traversal attempts
- readonly failures for every mutating operation
- cross-mount `rename` and `link` returning `EXDEV`
- open handle behavior after rename/unlink
- hardlinks within one writable host mount
- lookup count decrement and stale inode retirement
- readdir offsets under concurrent create/delete
- xattr success and unsupported-host behavior
- `access` enforcing readonly/write intent
- `flush`, `fsync`, and `release` handle cleanup
- SQLite-style WAL smoke on a real mounted composed-fs path in live VM
  validation, including concurrent host and guest writers against the same
  workspace database followed by `PRAGMA integrity_check`

VM integration tests should run the smoke commands above on:
- `q35 + composed fs`
- `microvm + composed fs`

## Deliberate V1 Compromises

The following compromises are acceptable for v1 if documented in the
implementation ticket:

- no live migration state support beyond default `SerializableFileSystem`
  unsupported behavior
- no special-device `mknod`
- no cross-mount hardlinks or renames
- no reliable host-coherent long-lived attribute/path cache
- `fallocate`, `copyfilerange`, `syncfs`, `ioctl`, and `poll` may be
  unsupported

These are not acceptable compromises:
- path-based traversal that can escape a mount root
- readonly policy that can be bypassed through a broader writable mount
- returning global `ENOSYS` for `access` and thereby making all future access
  checks succeed
- dropping open or looked-up inodes immediately after unlink/rename while the
  guest may still legally address them
- hiding file mounts from parent `readdir`
