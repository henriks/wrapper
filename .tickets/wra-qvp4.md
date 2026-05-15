---
id: wra-qvp4
status: closed
deps: []
links: []
created: 2026-05-15T08:56:06Z
type: task
priority: 3
assignee: Henrik Saksela
parent: wra-sne0
---
# Use thiserror for frontend launch errors

Replace hand-written Display, Error, and partial From impls for LaunchError in vm-frontend/src/launch.rs with thiserror. Current enum is still small, so this is a modest cleanup, but it prevents boilerplate drift as launch errors grow.

## Acceptance Criteria

LaunchError derives thiserror::Error or equivalent, manual Display/Error boilerplate is removed, and existing error text remains stable enough for tests and diagnostics. cargo test --manifest-path vm-frontend/Cargo.toml --offline passes.


## Notes

**2026-05-15T09:19:04Z**

LaunchError now derives thiserror::Error with per-variant display text matching the previous manual Display impl. Manual Error/Display boilerplate was removed. Validation: vm-frontend offline tests and fmt check pass.
