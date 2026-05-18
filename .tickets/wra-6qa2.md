---
id: wra-6qa2
status: closed
deps: []
links: [wra-ylfx]
created: 2026-05-18T10:35:51Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-emj5
tags: [cleanup, launch, shutdown, supervisor]
---
# Consolidate QEMU lifecycle before graceful shutdown

wra-ylfx needs graceful guest/QMP shutdown, but launch.rs currently has a stack of QEMU lifecycle wrappers: run_supervised_qemu_process_async, with_state, with_service, with_services, and with_services_and_shutdown. Adding graceful shutdown on top risks another adapter layer. First collapse QEMU process startup, service monitoring, supervisor shutdown, timeout, state writing, and forced kill fallback into one explicit lifecycle path.

## Design

Refactor launch.rs so QEMU lifecycle policy is represented once, with options for services and shutdown watches rather than multiple wrapper functions. The surviving path should make force kill a fallback after graceful guest/QMP shutdown timeout, not the primary shutdown behavior. Keep service cancellation and launch-state writing in one place.

## Acceptance Criteria

QEMU launch/shutdown has one primary lifecycle function or type; wrapper variants are deleted or trivially thin test helpers; force kill is only a documented fallback path ready for wra-ylfx; existing supervised launch tests pass; required validation is recorded before close.


## Notes

**2026-05-18T10:44:06Z**

Iteration 1 audit started: launch.rs currently layers run_supervised_qemu_process_async, run_supervised_qemu_process_with_state_async, run_supervised_qemu_process_with_service_async, run_supervised_qemu_process_with_services_async, and run_supervised_qemu_process_with_services_and_shutdown_async around spawn_supervised_qemu_process_with_state_async plus wait_for_qemu_or_service_failure. Next step should replace these wrappers with a single lifecycle options/config path while preserving state writes, service cancellation, shutdown watch, timeout, and force-kill fallback semantics.

**2026-05-18T10:48:05Z**

Iteration 2 progress: collapsed supervised QEMU lifecycle wrappers in vm-frontend/src/launch.rs into one SupervisedQemuLifecycle path plus run_supervised_qemu_lifecycle_async. Deleted run_supervised_qemu_process_async, with_state, with_service, with_services, with_services_and_shutdown wrapper functions and the finish_supervised_qemu_wait helper. Production launch now uses the lifecycle struct directly with services and supervisor shutdown receiver; tests were migrated to the same path. Verification so far: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --lib supervised_async_qemu; cargo test --manifest-path vm-frontend/Cargo.toml --lib supervised_qemu_with; cargo test --manifest-path vm-frontend/Cargo.toml --lib supervised_qemu_exit; cargo test --manifest-path vm-frontend/Cargo.toml --lib supervised_qemu_timeout; cargo check --manifest-path vm-frontend/Cargo.toml --lib --bin agentvm-frontend. Required validation still pending before close. No sudo appliance rebuild required.

**2026-05-18T10:52:55Z**

Iteration 3 completion validation: ./vm-frontend/validate.sh required passed with the consolidated SupervisedQemuLifecycle path. The forced-kill behavior is now centralized behind the single supervised lifecycle outcome handling, leaving one path for wra-ylfx to add graceful guest/QMP shutdown before forced termination. No sudo appliance rebuild required.
