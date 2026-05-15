---
id: wra-qno1
status: closed
deps: [wra-h6ga, wra-8gq1]
links: []
created: 2026-05-15T06:07:19Z
type: feature
priority: 0
assignee: Henrik Saksela
parent: wra-zfib
tags: [fs, virtiofs, locks]
---
# Implement composed-fs POSIX lock bridge

Implement real byte-range lock handling in composed-fs once virtiofsd exposes lock request details and the host primitive is selected. The bridge must preserve guest lock-owner semantics while enforcing conflicts through host-visible locks on the backing files, so host processes and guest processes coordinate on shared writable mounts. This is required for SQLite-bearing trees such as natural guest HOME/tool state because SQLite warns that broken or missing locks can corrupt databases under concurrent access.

## Design

Map FUSE GETLK/SETLK/SETLKW to host locks for regular files in writable host-backed sources. Track guest lock owners separately rather than collapsing all guests into the virtiofsd daemon process. Maintain per-owner/per-file host descriptors as needed. Implement read locks, write locks, unlocks, range normalization, blocking waits, cleanup on release/flush/forget where applicable, and useful error mapping. Read-only mounts should retain correct readonly behavior; unsupported file types/platform gaps must return explicit errors.

## Acceptance Criteria

composed-fs implements GETLK, SETLK, and SETLKW for regular files on supported writable host-backed sources; conflicting host and guest locks block or fail consistently; lock cleanup does not release unrelated guest-owner locks; unsupported cases fail explicitly; POSIX_LOCKS is still advertised only after validation and rollout ticket completion.


## Notes

**2026-05-15T06:16:28Z**

Implemented composed-fs lock bridge using Linux OFD locks. ComposedFs now keeps a LockTable keyed by (inode, handle, FUSE owner), opens a separate /proc/self/fd/<handle-fd> file description for each guest lock owner, maps FUSE FileLock start/end/type to libc flock start/len/type, and handles GETLK/SETLK/SETLKW via F_OFD_GETLK/F_OFD_SETLK/F_OFD_SETLKW. flush removes locks for the closing lock owner; release removes locks for the handle. init() still withholds POSIX_LOCKS until the validation/rollout tickets complete. Initial direct tests cover guest owner conflict, GETLK conflict reporting, host POSIX conflict, flush cleanup, and release cleanup. Verification: cargo test --manifest-path composed-fs/Cargo.toml --offline passed.

**2026-05-15T06:41:21Z**

Follow-up correction during validation: opening /proc/self/fd/<fd> did not provide an independent OFD suitable for per-owner locks; flush cleanup left locks held as long as the original handle stayed open. The bridge now stores the original host lock path plus dev/ino for each FileHandle, reopens that path for each guest owner, verifies dev/ino match, and uses that separate open file description for OFD locks. This preserves guest-owner isolation for normal path-stable files used by SQLite.
