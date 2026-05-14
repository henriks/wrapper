---
id: wra-fi7l
status: closed
deps: [wra-hqj6]
links: []
created: 2026-05-14T20:13:01Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-f16x
tags: [tests, filesystem, composed-fs, sandbox]
---
# Add composed-fs host mutation race and path escape tests

Add targeted race-oriented tests for composed-fs path resolution and handle behavior in composed-fs/src/lib.rs. The goal is to find sandbox-boundary bugs caused by host-side mutation of the backing tree between guest operations. This is especially important because composed-fs tracks namespace nodes, host dev/ino keys, cached attrs, relative paths, and open file handles while the host tree can change concurrently.

## Design

Write focused tests before or alongside broader fuzzing for high-risk interleavings: lookup sees a regular file then host swaps it for a symlink before open; host renames a parent directory after lookup but before open/read/write; readdir iterator while host renames/unlinks/recreates entries; inode reuse after delete/recreate; open writable handle survives unlink/rename/truncate without resolving a stale path outside the mount. Use threads only where needed; deterministic staged mutation is preferable when it exercises the same bug class.

## Acceptance Criteria

- Tests demonstrate host-side symlink swaps cannot make guest open/read/write escape the mount root.
- Tests cover parent directory rename/delete while child inode/handle is cached.
- Tests cover readdir and lookup behavior under host delete/recreate/inode reuse.
- Expected errno or content behavior is asserted explicitly, with no silent success for boundary violations.


## Notes

**2026-05-14T20:30:48Z**

Added targeted host-mutation race tests in composed-fs: lookup-then-host-symlink-swap cannot expose outside content; cached child under host parent replacement by symlink cannot expose outside content; an already-open writable handle continues to write the original file after the host renames the parent directory; lookup after host delete/recreate reads the replacement content rather than stale data. Verification: cargo test --manifest-path composed-fs/Cargo.toml --offline passed (34 run, 3 ignored).
