---
id: wra-8gq1
status: closed
deps: [wra-ftor]
links: []
created: 2026-05-15T06:07:11Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-zfib
tags: [fs, linux, sqlite, locks]
---
# Validate host byte-range lock primitive for guest-owner bridging

Before implementing the bridge, prove which Linux host lock primitive can safely represent guest locks while coordinating with ordinary host SQLite. A naive daemon-wide POSIX fcntl implementation is likely wrong because POSIX locks are process-associated: multiple guest processes would collapse into one host process owner, and close() behavior can drop unrelated locks. Candidate approach is per-guest-owner open-file descriptions plus Linux OFD byte-range locks, but it must be proven that these conflict as needed with host POSIX locks used by SQLite and that close/unlock behavior matches guest expectations.

## Design

Create focused host-only tests or small probes for POSIX fcntl locks, OFD locks, lock conflicts between the two, duplicated file descriptors, separate opens, close semantics, fork behavior, blocking locks, and GETLK conflict reporting. Use SQLite's documented requirements as the bar: all processes using the same database path must observe one coherent locking protocol. Document any kernel/version assumptions.

## Acceptance Criteria

The chosen host lock primitive and fd ownership model are documented with passing probes. The result explicitly states whether OFD locks conflict correctly with host POSIX SQLite locks on the supported Linux kernels, or identifies an alternate design if they do not.


## Notes

**2026-05-15T06:13:21Z**

Validated Linux OFD locks as the host primitive for the composed-fs guest-owner bridge. Added host-only tests proving: (1) an OFD write lock conflicts with a POSIX F_SETLK write lock attempted by a separate host process, which is the key SQLite coordination requirement; (2) separate open file descriptions on the same path conflict with each other and can represent distinct guest lock owners; (3) closing a dup of an owner's fd does not release the OFD lock while the owning open file description remains open. Command: cargo test --manifest-path composed-fs/Cargo.toml --offline ofd_ -- --nocapture passed.
