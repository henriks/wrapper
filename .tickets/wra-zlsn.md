---
id: wra-zlsn
status: open
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

