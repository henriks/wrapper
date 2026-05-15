---
id: wra-zfib
status: closed
deps: []
links: []
created: 2026-05-15T06:06:39Z
type: epic
priority: 0
assignee: Henrik Saksela
tags: [fs, virtiofs, sqlite, locks]
---
# Implement host-coherent POSIX byte-range locks for composed virtiofs

ComposedFs currently does not advertise POSIX_LOCKS and its lock hooks return EOPNOTSUPP because virtiofsd 1.13.3 exposes getlk/setlk/setlkw to FileSystem without the request details needed to translate guest locks. That was acceptable as an explicit gate, but shared writable guest HOME/tool state is expected to contain SQLite databases. SQLite documents that missing or broken filesystem locks can corrupt databases when multiple processes access the same DB concurrently: https://sqlite.org/howtocorrupt.html#_file_locking_problems. This epic makes byte-range locks a hard filesystem correctness requirement for shared writable host/guest mounts. The target is generic POSIX/FUSE correctness, not Codex-specific policy.

## Design

Implement at the virtiofs server/composed-fs layer. Avoid pretending locks work until guest fcntl requests are bridged to host-visible locks correctly. Likely path: patch or fork the Rust virtiofsd crate so FileSystem receives full FUSE lock request fields, then implement guest lock-owner tracking in composed-fs and map locks to Linux host byte-range locks in a way that coordinates with host SQLite. Prefer OFD locks internally if they can be proven to conflict correctly with host POSIX locks and preserve guest-owner semantics; otherwise document and choose the least surprising host lock primitive. Add live host+guest SQLite stress validation before advertising POSIX_LOCKS.

## Acceptance Criteria

The epic is complete when composed-fs advertises POSIX_LOCKS only after implemented lock semantics pass unit, integration, and live VM SQLite concurrency tests; host and guest SQLite processes can concurrently access the same WAL database through the shared mount without integrity_check failures; unsupported or platform-incomplete paths fail explicitly instead of silently succeeding; docs describe the supported locking semantics and validation workflow.


## Notes

**2026-05-15T06:42:00Z**

Epic implementation complete. Added a local virtiofsd API patch exposing FUSE lock request fields, implemented composed-fs POSIX byte-range locking through per-guest-owner host OFD lock descriptions, advertised POSIX_LOCKS, expanded lock correctness tests, and added live host+guest SQLite WAL validation. Important implementation details: OFD flock.l_pid must be zero for F_OFD_SETLK/F_OFD_SETLKW; /proc/self/fd reopening is not sufficient for independent owner descriptions, so the bridge reopens the path and verifies dev/ino. Live SQLite validation succeeded with a temporary patched rootfs artifact; the broader self-test still has an unrelated Docker TCP timeout.
