---
id: wra-zlsn
status: closed
deps: []
links: []
created: 2026-05-16T15:50:47Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-piqm
tags: [composed-fs, stability, security]
---
# Preserve composed-fs inode identity across host path replacement

Problem:
A looked-up host-backed FUSE inode stores mount plus relative path but does not retain and validate the expected host dev/ino identity for later operations. If the host deletes and recreates the path before guest forget, stale inode operations can follow the replacement path.

Relevant code:
- composed-fs/src/lib.rs:206 defines Host node shape.
- composed-fs/src/lib.rs:789-833 maps HostKey dev/ino to backend inode but does not keep identity in NodeKind.
- composed-fs/src/lib.rs:917-923 only decrements lookup counts.
- composed-fs/src/lib.rs:1372-1388 opens host file paths for existing inodes.

Impact:
A still-live guest inode can read, write, getattr, setattr, or xattr a different host file than the one originally looked up. This violates inode identity and can expose or mutate replacement files.

Recommended fix:
Store expected host identity on host/shadow nodes and validate stat/open fstat before using a path for an existing inode. Return ESTALE or cached attrs for stale old inodes, and allocate a new backend inode for replacement files.

Validation:
- Add host delete/recreate without forget, then assert old inode cannot open/read/write replacement and new lookup gets a distinct inode.
- Cover parent replacement and hardlink/inode reuse cases.
- Run composed-fs tests, relevant fuzz targets, and live filesystem validation before closing.


## Notes

**2026-05-18T09:39:45Z**

Iteration 40: started as next composed-fs correctness prerequisite for cleanup child wra-8xsb. Target is to store host dev/ino identity on Host nodes and validate opened/stat'ed paths before using cached inodes, so replacement paths get new inodes and stale old inodes fail closed.

**2026-05-18T09:46:56Z**

Implemented host inode identity preservation. Host nodes now store HostIdentity (dev/ino), host inode map keys by that identity, and stale cached Host paths validate identity before getattr/stat, open, setattr, readlink/access, link source use, xattr operations, and lock reopen. Replacement paths allocate a new backend inode while stale old inodes fail with ESTALE. Added stale_host_inode_does_not_follow_replacement_path regression. Validation passed: cargo test --manifest-path composed-fs/Cargo.toml --offline -- --nocapture; ./vm-frontend/validate.sh required (/tmp/pi-bash-5e4e798ccd25797a.log). During validation, existing parent-symlink tests were updated to accept ESTALE as the stronger fail-closed outcome, and lock-owner regression now retries briefly after flush to avoid transient OFD lock release timing.
