---
id: wra-b5t2
status: closed
deps: [wra-qno1]
links: []
created: 2026-05-15T06:07:25Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-zfib
tags: [fs, tests, locks]
---
# Add composed-fs byte-range lock correctness tests

Extend the fast and integration test sets to cover generic POSIX byte-range locking semantics in composed-fs. Current tests only assert that lock operations are not advertised and fail explicitly. Once the bridge exists, tests must prove ordinary lock behavior and the tricky POSIX/OFD ownership cases that SQLite depends on indirectly.

## Design

Add tests around non-overlapping and overlapping ranges, shared read locks, write lock exclusion, unlock subranges, GETLK conflict reporting, blocking SETLKW wakeup, release/close cleanup, multiple guest lock owners on the same file, host-vs-guest conflicts, read-only source behavior, file replacement/rename/unlink edge cases where applicable, and stress interleavings. Prefer reusable helpers over SQLite-specific shortcuts for this layer.

## Acceptance Criteria

The composed-fs test suite fails against the old EOPNOTSUPP implementation and passes against the real bridge. Tests cover both guest-owner internal conflicts and host process lock conflicts. Existing adversarial/path/read-write tests continue to pass.


## Notes

**2026-05-15T06:18:26Z**

Expanded composed-fs lock correctness coverage. New tests cover host primitive assumptions, guest owner conflicts, GETLK conflict reporting, host POSIX conflicts in both directions, shared read locks with write exclusion, subrange unlock preserving surrounding locked bytes, blocking SETLKW wakeup after unlock, readonly-handle behavior, flush owner cleanup, and release handle cleanup. Verification: cargo test --manifest-path composed-fs/Cargo.toml --offline passed with 46 normal tests, 3 existing ignored stress tests.
