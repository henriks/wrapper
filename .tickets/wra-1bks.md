---
id: wra-1bks
status: closed
deps: [wra-gx6d]
links: []
created: 2026-05-15T10:37:27Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-neci
---
# Guarantee frontend and QEMU cleanup on self-test failure paths

run_self_test can fail after launch but before normal termination during readiness wait, published payload ping, payload execution, SQLite checks, Docker checks, or other post-launch assertions. Earlier failed runs left stale QEMU holding docker-data.raw. RunningFrontend currently relies on explicit terminate calls rather than a tested cleanup guard.

## Design

Refactor launch/self-test lifecycle so every post-launch failure path terminates QEMU and helper tasks. Consider a Drop guard or scoped runner around RunningFrontend. Add fake-QEMU/fake-runner tests for readiness timeout, publish ping failure, payload failure, host SQLite failure, termination failure, stale socket cleanup, state.json status, and repeated runs after failure.

## Acceptance Criteria

No self-test failure path leaves a QEMU process or disk lock behind. Tests cover early post-launch errors and verify cleanup/state/log behavior. Live validation can be rerun immediately after an induced failure without manual process cleanup.


## Notes

**2026-05-15T10:55:53Z**

Started and surveyed lifecycle code. RunningFrontend in vm-frontend/src/launch.rs owns QEMU child and helper shutdown flag, but has no Drop guard; run_self_test in vm-frontend/src/main.rs calls start_frontend_with_policy then performs readiness/publish/payload/SQLite checks before an explicit running.terminate(). Any ? before terminate can leak QEMU/helpers and leave disk locks. Next step: add a cleanup guard or make RunningFrontend Drop terminate QEMU, then add fake process/self-test lifecycle tests for readiness/ping/payload failures.

**2026-05-15T10:58:17Z**

Implemented RunningFrontend cleanup guard: Drop now sets helper shutdown flag, kills/reaps an un-finished QEMU child, and writes state.json status=terminated; wait()/terminate() mark the runner finished so Drop does not overwrite normal exit/explicit termination state. Added fake-QEMU tests in vm-frontend/src/launch.rs for drop cleanup killing/reaping child and state, wait not overwritten by Drop, and explicit terminate reaping child. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline launch::tests:: -- --nocapture passed; validate docs/fmt/fast/fuzz-check passed. host-live still blocked by wra-pssg stale artifact gate until sudo ./docker/build-appliance.sh is run.

**2026-05-15T11:02:12Z**

Expanded cleanup coverage: added tests for wait timeout killing/reaping fake QEMU and writing timed_out state, repeated runner after Drop cleanup updating state on second run, and stale socket cleanup removing existing paths/ignoring missing. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline launch::tests:: -- --nocapture passed. A subsequent validate fast run initially hit a transient composed-fs POSIX lock WouldBlock in composed_lock_bridge_flush_and_release_cleanup_owner_locks; targeted rerun passed and full validate fast rerun passed.

**2026-05-15T11:30:00Z**

After appliance rebuild, ./vm-frontend/validate.sh required passed end-to-end with host-live self-test ok. Combined evidence: fake-QEMU cleanup tests cover Drop cleanup, wait timeout kill/reap, repeated runs after cleanup, stale socket cleanup, and explicit terminate; live self-test exercised frontend/QEMU lifecycle on rebuilt artifacts without stale locks.
