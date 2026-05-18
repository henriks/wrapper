---
id: wra-rleu
status: closed
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


## Notes

**2026-05-18T09:32:39Z**

Prerequisite wra-qdte is closed. Host traversal now requires openat2 and fails closed with EOPNOTSUPP on ENOSYS/EINVAL rather than falling back to openat; rleu can route readlink/access through confined parent resolution without preserving the old weak fallback.

**2026-05-18T09:34:49Z**

Iteration 39: started now that wra-qdte removed the weak openat fallback. Implementation target is the smallest consolidation: route readlink/access through existing confined parent resolver (open_beneath_with_mode via with_parent_dir) instead of adding a separate traversal path.

**2026-05-18T09:38:00Z**

Implemented confined parent resolution for readlink/access. readlink_beneath now opens the parent via with_parent_dir/open_beneath_with_mode before readlinkat on the leaf; access_beneath does the same for non-root paths and keeps explicit root access handling. Added parent-symlink replacement regressions for cached readlink and access. Validation passed: targeted readlink/access tests, cargo test --manifest-path composed-fs/Cargo.toml --offline -- --nocapture, and ./vm-frontend/validate.sh required (/tmp/pi-bash-970b7b9667685e04.log).
