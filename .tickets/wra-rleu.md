---
id: wra-rleu
status: open
deps: [wra-qdte]
links: []
created: 2026-05-16T15:50:47Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [composed-fs, security]
---
# Route composed-fs readlink and access through confined parent resolution

Problem:
readlink and access call readlinkat/faccessat directly with the mount root fd and full relative path, bypassing the safe traversal helper used elsewhere.

Relevant code:
- composed-fs/src/lib.rs:1451-1462 readlink path.
- composed-fs/src/lib.rs:1721-1743 access path.
- composed-fs/src/lib.rs:2416 and 2439 helper implementations.

Impact:
If a cached parent path is replaced with a symlink, readlink/access can probe outside the mount root. This is primarily an information leak/permission-probe issue and breaks the confinement invariant.

Recommended fix:
Resolve the parent directory with the same confined traversal path used by mutating operations, then call readlinkat/faccessat on the leaf.

Validation:
- Extend existing parent-symlink-replacement tests to cover readlink and access, not just open.
- Include fallback-mode coverage if the traversal fallback ticket changes helper behavior.

