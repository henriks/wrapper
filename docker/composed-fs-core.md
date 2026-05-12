# ComposedFs Namespace Core

Ticket: `wra-46m5`

Date: 2026-05-12

## Outcome

The repo-local `composed-fs/` backend now has the first real namespace core on
top of the scaffold:

- mount roots are opened on startup as host file descriptors with `O_NOFOLLOW`
- host-backed lookup/getattr/readdir use fd-relative `openat2`
- host descendants are identified by `(mount_index, dev, ino)` and reused for
  hardlinks inside a mount
- `lookup` increments per-node lookup counts and `forget`/`batch_forget`
  decrement them
- nested manifest mount points create overlay directories that merge host
  directory entries with mounted boundaries
- unsafe guest names such as `..` are rejected before host traversal

The implementation uses `openat2` with `RESOLVE_IN_ROOT` and
`RESOLVE_NO_MAGICLINKS` for host-relative traversal. If `openat2` is not
available, it falls back to `openat` after strict component validation. That
fallback is weaker against concurrent replacement of intermediate path
components and should be revisited before treating older kernels as supported.

## Current Boundaries

This ticket intentionally stops at the traversal and identity core. The
following behavior is still owned by later tickets:

- read/write/open/create/mkdir/unlink/rename/link and other mutating FUSE
  operations
- readonly policy enforcement for writes
- open handle tables for files and directories
- stale inode retirement after unlink/rename plus open-handle release
- cross-mount `EXDEV` behavior
- xattr, statfs, access, readlink, setattr, and fsync semantics

Mount root host paths are opened with `O_NOFOLLOW`, so final symlink mount roots
are rejected. Intermediate host path resolution is still delegated to the host
path passed in the manifest; the wrapper should continue resolving and
validating manifest sources before starting the backend.

## Tests

The unit tests cover:

- synthetic mount lookup and host child lookup
- `(mount, dev, ino)` inode reuse for hardlinks
- lookup-count decrement through `forget`
- nested overlay readdir merging a host entry with a mounted boundary
- rejection of unsafe lookup components
