---
id: wra-qdte
status: closed
deps: []
links: []
created: 2026-05-16T15:50:47Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [composed-fs, security, stability]
---
# Fail closed or implement strict safe-traversal fallback without openat2

Problem:
The safe traversal helper prefers openat2 with RESOLVE_IN_ROOT, but silently falls back to plain openat on ENOSYS/EINVAL. The fallback validates components but does not prevent intermediate symlink traversal out of the mount root.

Relevant code:
- composed-fs/src/lib.rs:2251-2283 implements open_beneath_with_mode.
- composed-fs/src/lib.rs:2266-2272 uses openat2 with IN_ROOT/NO_MAGICLINKS.
- composed-fs/src/lib.rs:2274-2280 falls back to openat.
- docker/composed-fs-core.md documents the intended confinement model.

Impact:
Older kernels or incompatible openat2 behavior can degrade path confinement and allow intermediate symlink escapes.

Recommended fix:
Prefer fail-closed with a documented minimum kernel, or implement a strict fallback resolver that opens each component with O_PATH/O_NOFOLLOW and pins parent fds before final operations.

Validation:
- Add an injectable resolver mode forcing fallback.
- Test intermediate symlink escapes and concurrent parent replacement.
- Update docs if the minimum kernel/support contract changes.


## Notes

**2026-05-18T09:28:15Z**

Iteration 38: started as the next composed-fs correctness prerequisite for cleanup child wra-8xsb. Chosen approach is fail-closed on ENOSYS/EINVAL openat2 unavailability rather than keeping the weaker openat fallback, because cleanup direction favors deleting unsafe fallback behavior over adding a parallel resolver.

**2026-05-18T09:32:39Z**

Implemented fail-closed host traversal when openat2 is unavailable: ENOSYS/EINVAL from openat2 now returns EOPNOTSUPP instead of falling back to plain openat. Deleted the unsafe fallback behavior while preserving live create compatibility by masking FUSE file-type bits out of openat2 create mode. Added tests open_beneath_fails_closed_when_openat2_is_unavailable and open_beneath_masks_file_type_bits_from_create_mode; updated docker/composed-fs-core.md. Validation passed: cargo test --manifest-path composed-fs/Cargo.toml --offline -- --nocapture; ./vm-frontend/validate.sh required (/tmp/pi-bash-8bea82967fc804e6.log). First required attempt failed live-smoke because create mode included file-type bits and openat2 returned EINVAL after fallback deletion; fixed by masking mode to permission bits.
