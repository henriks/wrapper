---
id: wra-bcvj
status: closed
deps: []
links: []
created: 2026-05-16T15:50:47Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-piqm
tags: [composed-fs, locks, sqlite]
---
# Fix composed-fs POSIX lock owner semantics across handles

Problem:
The lock bridge keys lock state by inode, handle, and owner. For the same guest lock owner on the same file through different handles, separate OFD descriptions can conflict with each other, and flush only removes locks for one handle/owner tuple.

Relevant code:
- composed-fs/src/lib.rs:282-292 defines LockKey/LockTable.
- composed-fs/src/lib.rs:1004-1029 opens one lock fd per inode/handle/owner.
- composed-fs/src/lib.rs:1031-1045 removes locks by handle.
- composed-fs/src/lib.rs:1674-1683 flush removes one handle/owner tuple.
- third_party/virtiofsd/src/filesystem.rs lock-owner semantics are the protocol boundary.

Impact:
Locks for the same guest owner but different handles can falsely conflict, deadlock, or leave stale host-visible locks. SQLite and other lock-sensitive workloads can see incorrect EAGAIN or stale lock behavior.

Recommended fix:
Key lock state by underlying file identity plus owner, not handle. Share one lock fd per owner/file across handles and make flush/release cleanup match FUSE/POSIX owner semantics.

Validation:
- Open the same file twice with the same owner and set overlapping locks through both handles.
- Unlock through one path and verify host and guest conflict behavior.
- Extend lock fuzz/property operations to include multiple handles per inode.


## Notes

**2026-05-18T09:25:59Z**

Implemented lock owner sharing by keying bridged POSIX locks by underlying host file identity (dev/ino) plus guest owner instead of per-handle. Added regression for same guest owner across two handles and extended the lock proptest to exercise operations through multiple handles. Validation passed: cargo test --manifest-path composed-fs/Cargo.toml --offline -- --nocapture; ./vm-frontend/validate.sh required (/tmp/pi-bash-de557005de254a02.log).
