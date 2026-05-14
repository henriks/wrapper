---
id: wra-hqj6
status: closed
deps: [wra-ay63]
links: []
created: 2026-05-14T20:12:53Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-f16x
tags: [tests, fuzzing, filesystem, composed-fs]
---
# Expand composed-fs property model beyond flat files

Expand the model-based composed-fs tests in composed-fs/src/lib.rs beyond the current flat-file operation model. The filesystem is a central sandbox boundary: the guest namespace must stay inside declared host mount roots while preserving FUSE-like handle, inode, and metadata semantics. Current property/stress tests cover flat create/read/rename/unlink/host-put/readdir sequences on a single rw mount. Existing example tests cover readonly rejection, symlink escape, cross-mount EXDEV, open handles after unlink/rename, inode reuse, xattrs, mknod, and nested overlay readdir.

## Design

Build a richer operation model with nested directories, mixed file/dir rename, rename-over-file/dir, mkdir/rmdir, symlink entries created by the host, open handles surviving unlink/rename/truncate, cross-mount attempts, and mixed ro/rw mount trees. Keep operation traces reproducible in failure output. The model should compare composed-fs behavior against expected host-tree state where possible, and explicitly assert expected errno for protected/readonly/cross-mount cases.

## Acceptance Criteria

- Generated operation sequences include nested dirs, file/dir renames, unlink/rmdir, open/read/write/release, host-side mutations, and readdir checks.
- Model covers at least one rw workspace mount plus one readonly or second mount boundary.
- Failures print seed/case and operation trace.
- Invariants assert no path escapes and no writes through readonly mounts.


## Notes

**2026-05-14T20:29:18Z**

Added a second composed-fs property model for nested operation sequences. It drives composed-fs and an independent host oracle through generated mkdir, create/write/release, read, rename, unlink, rmdir, host-put, host-mkdir, and readdir operations across nested paths. The model also includes explicit readonly create probes, cross-mount rename probes, and host-created symlink escape probes. Added a fast non-ignored proptest plus ignored proptest_nested_operation_sequences_stress with a documented command. Verification: cargo test --manifest-path composed-fs/Cargo.toml --offline passed (30 run, 3 ignored). Implementation insights: repeated lookup probes naturally increase lookup_count, so cross-mount checks should focus on EXDEV/no target; component-by-component composed-fs path resolution can return ENOENT where a single host syscall returns ENOTDIR, so the oracle treats those path-resolution errors as equivalent.
