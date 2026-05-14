---
id: wra-x8pk
status: open
deps: []
links: []
created: 2026-05-14T21:45:52Z
type: bug
priority: 0
assignee: Henrik Saksela
parent: wra-yz27
tags: [composed-fs, locks, sqlite]
---
# Implement or explicitly gate POSIX lock semantics for composed-fs

ComposedFs currently documents POSIX locks as a deliberate v1 deferral. That is dangerous for any guest-visible path that contains applications relying on fcntl/POSIX byte-range locks, especially SQLite databases in Codex state. If guest processes receive ENOSYS/success-like behavior or otherwise non-coherent lock behavior, host and guest processes may concurrently write data that assumes lock exclusion.

This ticket should either implement correct lock forwarding/semantics for getlk/setlk/setlkw through composed-fs or add explicit gating so lock-dependent shared host paths are not exposed read-write through composed-fs until correct locking exists.

Relevant code/docs:
- plan.md and docker/filesystem-semantics-baseline.md list POSIX locks getlk/setlk/setlkw as deferred.
- composed-fs/src/lib.rs implements many file operations but lock owner arguments are currently unused in read/write/flush/release paths, and lock operations need investigation at the FileSystem trait boundary.
- Codex host state contains SQLite databases and WAL/SHM files that normally rely on filesystem locking.

## Design

- Audit the virtiofsd FileSystem trait lock-related methods and current default behavior.
- Decide whether composed-fs can correctly forward locks to host fcntl locks for host-backed files, including lock owner mapping and release behavior.
- If full support is not feasible immediately, reject or avoid read-write sharing of known lock-dependent paths such as SQLite-bearing app state.
- Add tests that exercise conflicting locks from guest-side operations and, where feasible, host-side processes.

## Acceptance Criteria

- Lock-dependent shared paths are not exposed read-write without a documented and tested lock story.
- Tests cover POSIX lock behavior or the explicit rejection/gating behavior.
- Docs update the V1 compromises section to reflect the actual policy.
- Any unsupported lock operation fails in a way that does not let applications silently assume locking works when it does not.

