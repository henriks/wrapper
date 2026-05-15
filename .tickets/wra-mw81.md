---
id: wra-mw81
status: closed
deps: [wra-easf]
links: []
created: 2026-05-15T08:56:41Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-sne0
---
# Migrate composed-fs open and path syscalls to rustix

First implementation slice after the rustix spike: migrate the open/path-resolution syscall family in composed-fs/src/lib.rs to rustix where appropriate. Target areas include openat2/openat and path resolution helpers that protect against symlink and magic-link escapes. Preserve RESOLVE_IN_ROOT, RESOLVE_NO_MAGICLINKS, O_NOFOLLOW, O_PATH, and existing fallback/error behavior exactly.

## Acceptance Criteria

Open/path-resolution helpers use rustix where supported, with any remaining libc uses documented. Symlink escape, magic-link, host mutation, missing path, and readonly tests still pass. cargo test --manifest-path composed-fs/Cargo.toml --offline passes. Ticket notes identify the next syscall family to migrate.


## Notes

**2026-05-15T09:20:09Z**

Implemented the first rustix open/path slice in composed-fs/src/lib.rs. open_beneath_with_mode now uses rustix::fs::openat2 with IN_ROOT|NO_MAGICLINKS and falls back to rustix::fs::openat only for ENOSYS/EINVAL, preserving relative-path validation and caller-supplied O_NOFOLLOW/O_PATH/O_DIRECTORY/O_CLOEXEC flags. Remaining libc path/mutation syscalls are documented in wra-easf for later slices. Validation: composed-fs offline tests and fmt check pass.
