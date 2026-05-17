---
id: wra-n0fe
status: closed
deps: [wra-9glk, wra-r070, wra-rjt4]
links: [wra-zqci, wra-jkeg, wra-xcvq, wra-yl7i]
created: 2026-05-17T10:19:05Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, supervisor]
---
# Option 1: introduce Tokio application edge and supervisor skeleton

Introduce one Tokio runtime at the agentvm application edge and add an async supervisor skeleton without converting vmnet internals yet. The supervisor should be the owner of process and service lifecycle rather than launch.rs spawning detached threads coordinated by AtomicBool.

## Design

Use Tokio features only where needed: rt-multi-thread, macros, net, process, signal, sync, time, io-util, fs as required. Add a LaunchSupervisor or equivalent that owns task handles, cancellation, readiness channels, state writing, shutdown policy, and typed task results. Keep composed-fs on dedicated blocking threads or spawn_blocking initially.

## Acceptance Criteria

The binary can enter an async run path, supervisor APIs exist with typed errors and cancellation, no long-lived service is newly detached, and existing launch behavior is still covered by tests before deeper conversion.


## Notes

**2026-05-17T10:20:56Z**

Coordinate with wra-yl7i. The supervisor skeleton should be designed as the eventual backend for a project-local control socket: VM lifecycle, payload sessions, diagnostics, status/log events, shutdown semantics, and client attach/detach should have explicit ownership boundaries even if the socket protocol is implemented later.

**2026-05-17T10:28:44Z**

Async-boundary refinement: supervisor should own composed-fs/config-fs as blocking services under Tokio lifecycle supervision, not force their internals onto Tokio. Use dedicated blocking threads/spawn_blocking or process ownership with readiness/failure/shutdown reporting. Avoid running blocking filesystem request work on core Tokio workers.

**2026-05-17T12:22:32Z**

Starting after wra-rjt4 closure. Plan: introduce the smallest Tokio application edge/supervisor skeleton first, preserving existing synchronous launch behavior. Add Tokio as a direct vm-frontend dependency only with needed features, expose typed supervisor plan/task result/cancellation APIs, and avoid moving composed-fs/vmnet internals onto Tokio core workers in this slice.

**2026-05-17T12:24:21Z**

Iteration 28 initial Tokio/supervisor slice: added Tokio as a workspace dependency for vm-frontend with required edge/supervision features and fetched it so offline validation can use the updated lockfile. The binary now enters an async application edge by constructing a multi-thread Tokio runtime and blocking on run_cli_async, while preserving the existing synchronous run_cli command behavior. Added vm-frontend/src/supervisor.rs with LaunchSupervisor skeleton, typed task names/kinds/status/results, watch-channel shutdown/cancellation, and tests verifying blocking composed-fs/config-fs boundaries, async shutdown observation, and planned task results without spawning services. Validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervisor -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline argv0_no_longer_selects_wrapper_or_tool_behavior -- --nocapture.

**2026-05-17T12:26:14Z**

Iteration 29 supervisor ownership/readiness slice: extended LaunchSupervisor with per-task watch-backed status state, task_status/task_statuses queries, task status subscriptions, and SupervisorTaskController handles for marking starting/ready/finished/failed/cancelled with typed errors. This gives future supervised blocking services, vmnet, and qemu owners explicit readiness/failure/cancellation reporting without converting internals yet. Added tests for readiness/failure publication and one controller per planned task. Validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervisor -- --nocapture.

**2026-05-17T12:28:02Z**

Iteration 30 supervisor run-plan/offline validation: added a no-spawn run_until_shutdown API that waits for supervisor cancellation, preserves terminal task statuses, and marks unfinished tasks cancelled with the shutdown reason. This is enough skeleton surface for wra-n0fe: async binary edge exists, supervisor APIs expose typed task ownership/readiness/failure/cancellation, and no long-lived services are newly detached or converted internally. Validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervisor -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check; cargo test --manifest-path vm-frontend/Cargo.toml --offline.

**2026-05-17T12:28:32Z**

Iteration 31 reflection: wra-n0fe has met the intended foundational scope: the binary enters through a Tokio runtime, launch command behavior remains synchronous/unchanged for now, and LaunchSupervisor exposes typed task specs, per-task status/watch subscriptions, controller handles, shutdown cancellation, and a no-spawn run summary. The approach of landing a minimal skeleton before converting runtime internals is working: it creates stable ownership/readiness/failure seams without moving composed-fs/vmnet work onto Tokio core workers. Main tradeoff is that current production launch.rs still owns detached thread spawning; deeper conversion belongs to follow-on tickets like wra-zqci/wra-gq8e. Next step is required live-capable validation; if it passes with no appliance rebuild request, close wra-n0fe.

**2026-05-17T12:31:45Z**

Required validation passed for Tokio edge/supervisor skeleton: ./vm-frontend/validate.sh required completed end-to-end with docs/fmt/offline tests, offline guest service tests, fuzz target compilation, live-smoke, and live-setup-tools. No appliance rebuild was requested. Scope intentionally stops at async binary edge plus no-spawn supervisor ownership/readiness/failure/cancellation APIs; production long-lived service conversion remains for dependent tickets.
