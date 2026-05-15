---
id: wra-eqmj
status: closed
deps: []
links: []
created: 2026-05-15T08:55:41Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-sne0
---
# Replace project flock calls with fs4

Replace direct unsafe libc::flock calls in vm-frontend/src/main.rs ProjectLock::acquire and reset_project with fs4 FileExt locking APIs. The current behavior is exclusive nonblocking project lock using LOCK_EX | LOCK_NB and unlock on drop. Candidate crate: fs4.

## Acceptance Criteria

Unsafe flock calls are removed from vm-frontend/src/main.rs or isolated behind fs4-backed helper code. Busy project lock errors remain actionable and reset still refuses when a VM is active. Relevant wrapper/reset tests pass and cargo test --manifest-path vm-frontend/Cargo.toml --offline passes.


## Notes

**2026-05-15T09:18:33Z**

Replaced unsafe libc::flock calls in ProjectLock/reset_project with fs4::FileExt::try_lock/unlock through acquire_project_file_lock. Busy-lock messages remain specific for active sandbox and reset refusal. Validation: vm-frontend offline tests and fmt check pass.
