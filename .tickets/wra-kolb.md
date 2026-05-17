---
id: wra-kolb
status: closed
deps: []
links: []
created: 2026-05-17T17:15:23Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, validation, flake]
---
# Fix flaky supervised blocking service late-failure test

During wra-662v required validation, vm-frontend offline test launch::tests::supervised_blocking_service_marks_failed_after_readiness failed nondeterministically. The test pre-created the readiness marker and used a blocking service closure that returned Err immediately, so spawn_supervised_blocking_service_until_ready could observe the service exit before the async readiness poll observed the ready path. This is a test race rather than intended production behavior. Fix by making the test service remain alive briefly after readiness so the helper deterministically returns a ready handle before handle.wait() observes the late failure.

## Acceptance Criteria

The flaky test is made deterministic, focused vm-frontend supervised_blocking_service tests pass repeatedly or at least in focused validation, and broader validation is rerun.


## Notes

**2026-05-17T17:36:12Z**

Implemented deterministic test fix by making the fake service sleep briefly after readiness before returning the late failure. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_blocking_service_marks_failed_after_readiness -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_blocking_service -- --nocapture. Not closing yet because the subsequent required validation run timed out before completing.

**2026-05-17T17:48:41Z**

Broader validation after deterministic test fix passed: cargo test --workspace --offline; cargo fmt --all -- --check. The fixed test and supervised_blocking_service group also passed focused validation earlier.
