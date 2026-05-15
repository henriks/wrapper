---
id: wra-zukd
status: closed
deps: [wra-x8pk, wra-ndxm]
links: []
created: 2026-05-14T21:47:18Z
type: bug
priority: 0
assignee: Henrik Saksela
parent: wra-yz27
tags: [composed-fs, sqlite, filesystem]
---
# Validate SQLite-style database workloads on composed-fs generically

SQLite/WAL/SHM behavior should be treated as a generic filesystem workload, not as Codex-specific policy. If composed-fs provides correct locking, mmap/shared-memory behavior where applicable, fsync/flush ordering, rename/unlink semantics, and cache coherency, then applications such as Codex should just work when they store state under normal shared paths.

This ticket adds generic validation for SQLite-style workloads on composed-fs and documents the supported contract. It must not add Codex-specific path handling.

Relevant context:
- docker/filesystem-semantics-baseline.md currently lists POSIX locks and reliable host-coherent long-lived cache as compromises/deferrals.
- composed-fs/src/lib.rs implements fsync/flush and many mutation operations, but SQLite-level behavior has not been validated as a workload.
- Host/guest shared application state can include SQLite databases in many tools, not only Codex.

## Design

- Add a generic SQLite smoke/regression test against a composed-fs-backed writable directory.
- Include concurrent access where possible: one writer/reader through the guest-facing FS path and one host-side process on the backing path.
- Exercise WAL mode, rollback journal mode if relevant, transaction commit, recovery after process exit, and lock contention behavior.
- Keep failures framed as composed-fs capability gaps, not app-specific policy.

## Acceptance Criteria

- A generic SQLite workload test exists for composed-fs or VM integration.
- The test covers lock contention and WAL/SHM sidecar behavior, or explicitly documents why a mode is unsupported.
- No Codex-specific code or manifest policy is introduced.
- Docs state the composed-fs support level for SQLite-style database workloads.


## Notes

**2026-05-14T21:51:16Z**

User requested broadening tests for POSIX filesystem behavior generally. Treat SQLite as one workload in a larger filesystem semantics test expansion; include lock, fsync/flush, rename/unlink, sidecar files, and concurrent access behavior where feasible.

**2026-05-14T21:58:52Z**

Added generic SQLite/WAL smoke to the live VM self-test payload using Python sqlite3 under guest HOME, and documented the same smoke in filesystem-semantics-baseline.md and validation-workflow.md. This is not Codex-specific; it validates a normal SQLite-style workload on the real mounted composed-fs path when live validation runs. Fast tests verify the self-test payload includes the sqlite3/WAL workload. Verification: cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo test --manifest-path composed-fs/Cargo.toml --offline.
