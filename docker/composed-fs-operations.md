# ComposedFs Operation Surface

Ticket: `wra-vy20`

Date: 2026-05-13

## Implemented Slice

The composed backend now implements the first practical v1 operation slice on
top of the namespace core:

- file handle allocation and release
- `open`
- `create`
- `read`
- `write`
- `flush`
- `fsync`
- `release`
- `mkdir`
- `mknod` for regular files and FIFOs
- `unlink`
- `rmdir`
- `rename`
- `link`
- `symlink`
- `readlink`
- `statfs`
- `access`
- `lseek`
- `setattr` for chmod/chown/truncate/timestamps
- xattrs: `getxattr`, `listxattr`, `setxattr`, `removexattr`
- `fsyncdir`
- cached metadata for host-backed inodes whose original path disappears after
  unlink

Readonly policy is enforced for write-intent operations in this slice. A
readonly manifest mount rejects creates, mutating opens, writes through readonly
handles, directory creation, link/rename/unlink/rmdir/symlink/mknod, and write
access probes with `EROFS`.

## Traversal Model

Mutating operations resolve the parent directory with the existing fd-relative
safe traversal and then call the corresponding `*at` syscall on a single
validated final path component. Cross-mount renames and hardlinks fail with
`EXDEV`.

## Documented Compromises

Stale host-backed inodes are retained in memory for the backend process
lifetime. If a host-backed path is unlinked after lookup, `getattr` can still
return the last cached attributes for that inode, but the node is not yet
retired when lookup counts and open handles both reach zero. This is acceptable
for the first v1 slice, but the full correctness test ticket should either add
retirement or convert the retention behavior into an explicit bounded cache.

The following FUSE operations intentionally remain deferred and currently use
the upstream trait defaults:

- POSIX locks: `getlk`, `setlk`, `setlkw`
- `ioctl`
- `bmap`
- `poll`
- `notify_reply`
- `tmpfile`
- `copyfilerange`
- `syncfs`
- `fallocate`

User impact: ordinary shell, Git, package-manager, and Docker bind-mount
workflows should not require these deferred operations. If integration testing
shows a real workload depends on one, add a focused ticket before making the
backend default.

The current tests are unit-level backend tests. Full guest-mounted virtio-fs
validation remains a later integration ticket.
