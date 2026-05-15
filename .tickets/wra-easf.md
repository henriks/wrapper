---
id: wra-easf
status: closed
deps: []
links: []
created: 2026-05-15T08:56:35Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-sne0
---
# Spike rustix migration for composed-fs syscalls

Plan a careful migration from hand-written libc syscall wrappers to rustix in composed-fs/src/lib.rs. Relevant functions span openat2/openat/mkdirat/mknodat/unlinkat/renameat2/linkat/symlinkat/readlinkat/faccessat/fstatvfs64/xattrs/fcntl and errno conversion around lines roughly 1784-2255, plus lock helpers around fcntl. This code is on the security boundary, especially openat2 RESOLVE_IN_ROOT and RESOLVE_NO_MAGICLINKS behavior, so do not do a broad rewrite without a plan.

## Acceptance Criteria

Ticket notes document which syscall families rustix supports cleanly, any APIs that still require libc, exact security invariants to preserve, and the recommended migration sequence. Follow-up implementation tickets are created for syscall families, each with targeted regression tests. No behavior-changing code rewrite is required for this spike.


## Notes

**2026-05-15T09:19:56Z**

Spike outcome: rustix supports the open/path-resolution slice directly via rustix::fs::openat2/openat, OFlags, Mode, ResolveFlags, and OwnedFd. Security invariants to preserve: validate_relative_path before any host syscall, RESOLVE_IN_ROOT and RESOLVE_NO_MAGICLINKS on openat2, O_CLOEXEC on every opened fd, O_NOFOLLOW/O_PATH callers preserved, and only ENOSYS/EINVAL fall back from openat2 to openat. Other syscall families still needing separate risk review: mkdirat/mknodat/unlinkat/renameat2/linkat/symlinkat/readlinkat/faccessat, fstatvfs64, setattr/xattrs, and OFD fcntl lock translation.
