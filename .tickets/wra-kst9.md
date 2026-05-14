---
id: wra-kst9
status: closed
deps: [wra-brsl, wra-x8pk]
links: []
created: 2026-05-14T21:46:03Z
type: bug
priority: 0
assignee: Henrik Saksela
parent: wra-yz27
tags: [codex, sqlite, state]
---
# Isolate Codex SQLite and WAL state from host-guest sharing

Host ~/.codex contains SQLite databases and sidecar files such as state_5.sqlite, logs_2.sqlite, *.sqlite-wal, and *.sqlite-shm. Sharing these files read-write with guest Codex is especially risky because SQLite expects precise locking, mmap/shared-memory, rename/fsync, and cache coherency semantics. Even if general Codex state sharing is narrowed, SQLite/WAL/SHM files need an explicit policy so they are never accidentally included in a writable shared mount.

This ticket is separate from the general ~/.codex policy because SQLite files have stricter filesystem requirements and need focused regression tests.

Relevant context:
- Host ~/.codex listing shows state_5.sqlite, state_5.sqlite-wal, state_5.sqlite-shm, logs_2.sqlite, logs_2.sqlite-wal, and logs_2.sqlite-shm.
- docker/filesystem-semantics-baseline.md currently calls out no POSIX lock implementation and no reliable host-coherent long-lived attribute/path cache as V1 compromises.
- composed-fs supports fsync and many operations, but the full SQLite-on-shared-FS contract has not been validated.

## Design

- Define a denylist or storage split for Codex SQLite/WAL/SHM files.
- Keep guest Codex SQLite/log state in guest-owned/project-local backing storage unless composed-fs proves the complete SQLite contract.
- Add manifest validation or policy tests that prevent these files from being included through a broad writable Codex mount.
- Include a focused SQLite smoke/regression test if any shared SQLite path remains possible.

## Acceptance Criteria

- Host Codex SQLite, WAL, and SHM files are not writable from the guest by default.
- Manifest generation tests prove Codex SQLite files are isolated or denied.
- A regression test covers concurrent host/guest or simulated guest access to SQLite-bearing state.
- Documentation states where guest Codex SQLite/log state lives and why it is not shared with host Codex.


## Notes

**2026-05-14T21:46:53Z**

Superseded after clarification: this framed the problem as Codex-specific SQLite/WAL policy. Do not add Codex semantics to wrapper code. Replace with generic SQLite/database filesystem-semantics validation for any shared writable directory.
