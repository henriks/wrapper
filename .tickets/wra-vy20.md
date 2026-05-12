---
id: wra-vy20
status: in_progress
deps: [wra-46m5, wra-30di]
links: []
created: 2026-05-11T20:43:42Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-umuv
tags: [virtiofs, filesystem, readonly]
---
# Implement ComposedFs v1 operation surface and readonly enforcement

Implement the v1 FileSystem operation surface required by the documented semantics baseline, including normal file/dir I/O and readonly policy enforcement for manifest ro mounts.

## Design

Expected operations include lookup, forget, getattr, setattr, access, opendir, readdir, releasedir, open, create, release, flush, fsync, read, write, statfs, readlink, symlink, mkdir, unlink, and rename unless the baseline documents an exception. Return EROFS for mutating operations across readonly boundaries, including truncating opens and rename into/out of readonly subtrees.

## Acceptance Criteria

The required v1 operation surface works against representative guest tooling; readonly bypass attempts fail; unsupported operations are explicitly documented with rationale and user impact.


## Notes

**2026-05-12T20:54:22Z**

Dependency insight from wra-30di: v1 operation surface is larger than the early plan. Required: init, lookup, forget/batch_forget, getattr, setattr, access, readlink, symlink, mkdir, create, mknod regular fallback or explicit EPERM for special nodes, unlink, rmdir, rename, link within same writable mount, open, read, write, flush, fsync, release, statfs, opendir, readdir, fsyncdir, releasedir, xattrs, and lseek. Do not return global ENOSYS for access, because the kernel treats that as permanent success. See docker/filesystem-semantics-baseline.md.

**2026-05-12T21:16:41Z**

Handoff from wra-46m5: namespace/traversal core is implemented and should not be duplicated. Continue by adding handle tables and the v1 FileSystem operation surface on top of existing NodeKind::{SyntheticDir, OverlayDir, MountRoot, Host}, MountRuntime root fds, host identity map, and lookup_count bookkeeping. Remaining explicit gaps from docker/composed-fs-core.md: read/write/open/create/mkdir/unlink/rmdir/rename/link, readonly policy enforcement, stale inode retirement after unlink/rename plus release, cross-mount EXDEV, access/readlink/setattr/statfs/xattrs/lseek/flush/fsync/fsyncdir.

**2026-05-12T21:17:22Z**

Started after wra-46m5 closure. Next implementation slice should add handle tables and implement open/create/read/write/flush/fsync/release plus readonly write-intent rejection using the existing MountRuntime access policy. The FileSystem trait signatures for these methods are in virtiofsd 1.13.3 src/filesystem.rs; read/write should use ZeroCopyWriter::read_from_file_at and ZeroCopyReader::write_to_file_at against host File handles.
