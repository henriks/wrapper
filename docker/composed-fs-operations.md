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

Readonly policy is enforced for write-intent operations in this slice. A
readonly manifest mount rejects creates, mutating opens, writes through readonly
handles, directory creation, link/rename/unlink/rmdir/symlink/mknod, and write
access probes with `EROFS`.

## Traversal Model

Mutating operations resolve the parent directory with the existing fd-relative
safe traversal and then call the corresponding `*at` syscall on a single
validated final path component. Cross-mount renames and hardlinks fail with
`EXDEV`.

## Remaining Work Before Closing `wra-vy20`

The following operation-surface items remain:

- `setattr` for chmod/chown/truncate/timestamps
- xattrs: `getxattr`, `listxattr`, `setxattr`, `removexattr`
- `fsyncdir`
- stale inode retirement after unlink/rename once lookup counts and open
  handles both drop
- broader readonly tests for every mutating operation
- expected-error documentation for deferred operations such as locks, ioctl,
  copyfilerange, syncfs, tmpfile, and fallocate

The current tests are unit-level backend tests. Full guest-mounted virtio-fs
validation remains a later integration ticket.
