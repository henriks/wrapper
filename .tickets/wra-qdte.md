---
id: wra-qdte
status: open
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

