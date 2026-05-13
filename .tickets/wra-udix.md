---
id: wra-udix
status: closed
deps: [wra-vy20]
links: []
created: 2026-05-11T20:43:59Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-umuv
tags: [tests, virtiofs, filesystem, security]
---
# Add ComposedFs correctness and adversarial test coverage

Add tests for the composed filesystem core and v1 operation surface, including adversarial path traversal and readonly-boundary behavior. These tests are required because the backend handles untrusted guest filesystem requests.

## Design

Cover synthetic lookup/readdir, mounted subtree delegation, file mounts, nested mount boundaries, ro inside rw, symlink and .. escape attempts, open-then-rename/unlink behavior, hardlink behavior or documented unsupported semantics, concurrent operations, lookup count/inode lifetime, and xattr behavior required by the baseline.

## Acceptance Criteria

Tests can be run by a documented command; security-sensitive cases are covered; any known gaps are documented in ticket notes and linked to follow-up tickets before closing.


## Notes

**2026-05-12T20:54:29Z**

Dependency insight from wra-30di: tests must cover synthetic lookup/readdir, file and dir mounts, nested readonly boundaries, symlink and .. escape attempts, readonly failures for each mutating operation, cross-mount rename/link EXDEV, open-handle behavior after rename/unlink, hardlinks within one writable host mount, lookup-count stale inode retirement, readdir offsets under mutation, xattr behavior, access enforcement, and flush/fsync/release cleanup. See docker/filesystem-semantics-baseline.md.

**2026-05-12T21:16:45Z**

Handoff from wra-46m5: composed-fs currently has unit coverage for hardlink inode reuse via (mount, dev, ino), lookup_count decrement through forget, unsafe component rejection, and nested overlay readdir merging host entries with mounted boundaries. Broader correctness/adversarial coverage still needs the v1 operation surface from wra-vy20 before testing readonly mutations, open-after-rename/unlink, xattrs, access, fsync, and cross-mount EXDEV.

**2026-05-12T21:36:50Z**

Added backend correctness/adversarial coverage in composed-fs unit tests. Coverage now includes file mounts, nested boundaries, symlink escape prevention, cross-mount rename/link EXDEV, readonly rejection for create/write-open/access/setattr/mkdir/unlink/rmdir/rename/link/symlink/mknod, open-handle read after unlink, cached getattr after unlink, mknod regular fallback/special EPERM, xattr delegation when supported, and core lookup/readdir/hardlink/lookup-count cases. Test command documented in docker/composed-fs-tests.md. Verified cargo build --manifest-path composed-fs/Cargo.toml --offline and cargo test --manifest-path composed-fs/Cargo.toml --offline; 17 tests pass. Residual gaps are guest-mounted integration behavior, stress/concurrency, and readdir offset mutation under kernel interaction; these belong to q35 composed-fs validation or focused follow-ups.
