---
id: wra-clxy
status: closed
deps: []
links: []
created: 2026-05-15T08:55:34Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-sne0
---
# Use wait-timeout for QEMU/process timeout waits

Replace the manual process timeout loop in vm-frontend/src/launch.rs around run_frontend_until_qemu_exit_with_policy_and_timeout. Current code uses try_wait plus sleep plus kill/wait. Candidate crate: wait-timeout, using ChildExt::wait_timeout while preserving the existing behavior that kills and then waits on timeout.

## Acceptance Criteria

Manual try_wait sleep polling for process timeout is removed from launch.rs. Timeout behavior, status reporting, and cleanup semantics are preserved. Existing launch tests pass and cargo test --manifest-path vm-frontend/Cargo.toml --offline passes.


## Notes

**2026-05-15T09:18:23Z**

Replaced manual try_wait/sleep timeout loop in vm-frontend/src/launch.rs with wait_timeout::ChildExt::wait_timeout. Timeout still kills QEMU and then waits to report the final status with timed_out=true. Validation: vm-frontend offline tests and fmt check pass.
