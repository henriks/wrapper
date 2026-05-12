---
id: wra-46m5
status: closed
deps: [wra-saox, wra-a9je, wra-30di]
links: []
created: 2026-05-11T20:43:42Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-umuv
tags: [virtiofs, filesystem, security]
---
# Implement ComposedFs namespace, inode, and safe traversal core

Implement the core ComposedFs namespace model: synthetic directories, mount roots, delegated host nodes, inode allocation/identity, lookup count tracking, and fd-relative safe host traversal beneath each mount root.

## Design

Use the documented filesystem semantics baseline. Avoid naive host path string concatenation. Enforce mount boundaries and prevent symlink or .. escape. Document the final inode identity policy, including any hardlink or rename compromises.

## Acceptance Criteria

Synthetic lookup/readdir, dir mounts, file mounts, nested boundaries, lookup/forget behavior, and safe traversal are implemented and covered by tests; semantic compromises are documented before closing.


## Notes

**2026-05-12T20:48:42Z**

Dependency insight from wra-zgpp: upstream FileSystem docs explicitly require lookup-count bookkeeping. Every returned Entry increments lookup count; forget decrements it; inodes with non-zero lookup count can continue receiving requests after unlink/rmdir/rename and open handles delay final forget until release/releasedir. Treat this as a core correctness requirement for ComposedFs, not a cache optimization.

**2026-05-12T20:54:16Z**

Dependency insight from wra-30di: namespace/inode core must support stable synthetic and mount-root inodes, lookup counts, stale inode retention while open/looked-up, and preferred host-backed identity based on (mount_id, dev, ino). Path-based identity is only acceptable as a documented temporary compromise with expected-failure tests for hardlink/open-after-rename behavior. Safe traversal must preserve nested ro/rw boundaries and prevent symlink/.. escape. See docker/filesystem-semantics-baseline.md.

**2026-05-12T20:57:23Z**

Dependency insight from wra-a9je: namespace core must build synthetic parents from manifest guest_path values and support first-class file mounts. Exact duplicate and ancestor-overlap resolution happens in the host wrapper, but backend must still defend against malformed manifests. Nested mount boundary metadata from the manifest is required for ro/rw enforcement and cross-boundary EXDEV behavior. See docker/composed-fs-manifest.md.

**2026-05-12T21:08:39Z**

Scaffold handoff from wra-saox: composed-fs/ now contains the buildable agentvm-composed-fs binary. Current ComposedFs only exposes manifest-derived synthetic parents and placeholder mount roots via init/lookup/getattr/opendir/readdir/releasedir; it deliberately does not delegate host-backed traversal or file operations. wra-46m5 should replace the placeholder mount-root behavior with safe host path resolution, mount precedence, inode/open-handle accounting, and escape prevention while preserving the existing CLI contract: --manifest, --socket-path, --tag, --thread-pool-size.

**2026-05-12T21:16:36Z**

Implemented namespace/inode/safe traversal core in composed-fs/. Host mount roots are opened as fds with O_NOFOLLOW; lookup/getattr/readdir delegate through fd-relative openat2 using RESOLVE_IN_ROOT and RESOLVE_NO_MAGICLINKS, with openat fallback only after strict component validation. Host nodes are keyed by (mount_index, dev, ino), so hardlinks within a mount reuse the backend inode. lookup increments lookup_count and forget/batch_forget decrement it. Nested mount boundaries are represented as overlay dirs that merge host entries and mounted children. Documented outcome and current boundaries in docker/composed-fs-core.md. Verified with cargo build --manifest-path composed-fs/Cargo.toml --offline and cargo test --manifest-path composed-fs/Cargo.toml --offline.
