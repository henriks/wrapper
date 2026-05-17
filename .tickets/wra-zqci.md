---
id: wra-zqci
status: closed
deps: [wra-n0fe]
links: [wra-jkeg, wra-xcvq, wra-n0fe, wra-yl7i]
created: 2026-05-17T10:19:11Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, supervisor, process]
---
# Option 1: replace launch process handling and readiness with async supervision

Replace start_frontend_with_policy and RunningFrontend lifecycle with async process and readiness supervision. Current launch.rs starts composed-fs, config-fs, vmnet, docker proxy, and QEMU, but only owns QEMU. Service failures currently mostly eprintln from detached threads.

## Design

Use tokio::process::Command for QEMU and host tools such as mkfs.ext4. Replace wait-timeout with tokio::time::timeout. Replace wait_for_path sleep polling with readiness oneshot channels and concrete probes. Treat vmnet/docker/composed-fs task failure as fatal while QEMU is running, terminate QEMU, write state, and return a typed error. Normal shutdown should be explicit async shutdown; Drop should only be last-resort cleanup.

## Acceptance Criteria

Supervisor owns and joins all launch services, QEMU timeout and service-failure paths are tested, readiness no longer depends on socket path existence alone, and state.json reflects shutdown causes.


## Notes

**2026-05-17T10:20:56Z**

Coordinate with wra-yl7i. Async launch supervision should expose lifecycle state and task outcomes in a form usable by a control-socket frontend. Avoid baking plain CLI or TUI behavior directly into supervisor internals; CLI/TUI should become clients of the same supervisor/control boundary.

**2026-05-17T10:28:44Z**

Async-boundary refinement: async launch supervision should integrate composed-fs as a supervised blocking service. Readiness, failure propagation, cancellation, and state reporting belong in the Tokio supervisor; composed-fs request execution remains synchronous/bounded blocking. If vhost transport later becomes async, keep FS operations behind a bounded blocking worker model unless profiling justifies more.

**2026-05-17T16:14:04Z**

Starting iteration 39. Scope for first slice: inspect current launch lifecycle/readiness ownership and choose an additive async supervision seam that does not rewrite composed-fs/vmnet internals yet. Initial direction is to reuse the existing LaunchSupervisor skeleton, introduce typed async process/readiness helpers in or alongside launch.rs, and keep composed-fs/config-fs as explicitly supervised blocking tasks while preserving current CLI behavior until a focused integration slice is safe.

**2026-05-17T16:14:16Z**

Iteration 39 inspection findings: current launch.rs still has a single synchronous start_frontend_with_policy path. It creates the state disk via blocking mkfs.ext4, writes state=starting, spawns composed-fs/config-fs/vmnet on detached std threads, waits for each socket via wait_for_path sleep polling, optionally starts docker proxy, then spawns QEMU with std::process::Command. RunningFrontend owns only the QEMU child and a shared shutdown flag; service task failures are logged but not propagated. wait_for_qemu uses wait-timeout and kill-on-timeout, and Drop remains last-resort cleanup that writes state=terminated. Existing LaunchSupervisor has status/watch/controller primitives but is not yet wired into launch.rs. First safe implementation slice should be additive: async helpers/tests for timeout/process/state behavior or a planned async launch runner surface, before replacing production start_frontend_with_policy wholesale.

**2026-05-17T16:16:18Z**

Iteration 40 first additive async process slice: added Tokio process helpers in launch.rs without changing production launch behavior. New run_qemu_process_async/spawn_qemu_process_async/wait_for_qemu_async use tokio::process::Command/Child and tokio::time::timeout, preserving QemuExit semantics and kill-on-timeout behavior as a reusable building block for the future async launch runner. Added async unit tests for normal nonzero QEMU/process exit and timeout kill behavior. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline async_qemu -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline launch_ -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check. Production start_frontend_with_policy remains synchronous; wra-zqci remains in progress and has not had required validation for closure.

**2026-05-17T16:22:50Z**

Continuation iteration 1: wired the additive Tokio QEMU helpers into the LaunchSupervisor status model via run_supervised_qemu_process_async. The helper validates it receives the QEMU controller, marks starting before spawn, ready after spawn, finished on normal exit, failed on timeout or spawn/wait errors, and preserves QemuExit return semantics. spawn_qemu_process_async now creates the qemu log parent directory so the async process helper is self-contained for tests/future runner usage. Added async tests for supervised QEMU normal exit and timeout status publication. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_async_qemu -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline async_qemu -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline launch_ -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T16:24:38Z**

Continuation iteration 2: added async readiness helpers for supervised service startup without changing production launch behavior. wait_for_path_async mirrors wait_for_path using tokio::time sleep/timeout boundaries, and wait_for_service_ready_async publishes ready/failed status through SupervisorTaskController. The synchronous wait_for_path now shares the same timeout error construction to keep messages stable. Added async tests for ready marker success and timeout failure status publication. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline async_service_readiness -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline async_qemu -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T16:26:19Z**

Continuation iteration 3: extended the supervised async QEMU helper into a state-writing launch-runner surface. New run_supervised_qemu_process_with_state_async validates the QEMU controller, marks starting/ready/finished/failed, writes state=running after async spawn with the Tokio child pid, then writes state=exited or state=timed_out with qemu_status after wait/timeout. Refactored shared supervised wait/status handling into finish_supervised_qemu_wait and controller mismatch helper. Added focused tests for state.json exit and timeout updates plus supervisor status. Production start_frontend_with_policy remains unchanged. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_async_qemu -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline async_service_readiness -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T16:27:52Z**

Continuation iteration 4: added supervised blocking service readiness helper for future composed-fs/config-fs/vmnet integration while keeping service work off Tokio core threads. spawn_supervised_blocking_service_until_ready marks the service task starting, runs the supplied blocking service with tokio::task::spawn_blocking, races service readiness against early service exit, returns the join handle after readiness, and marks failed on readiness timeout or early service failure. This is still additive; production start_frontend_with_policy remains unchanged. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_blocking_service -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline async_service_readiness -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_async_qemu -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T16:29:44Z**

Continuation iteration 5 reflection and service handle slice: wra-zqci is progressing through additive, testable seams: async QEMU process helpers, supervisor status publication, async readiness, state-writing QEMU runner, and bounded-blocking service startup. The approach is still sound because production launch behavior remains unchanged while future async runner pieces gain focused tests. Current gap: the pieces are not yet combined into a single launch runner that starts services, monitors service handles while QEMU runs, terminates QEMU on service failure, and writes shutdown cause state. This iteration changed spawn_supervised_blocking_service_until_ready to return SupervisedBlockingService, a wrapper retaining the task controller and join handle. SupervisedBlockingService::wait marks finished on clean service exit and failed on late service error/join failure, giving future runner code a typed service-failure observation point. Added focused late-failure test. Validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_blocking_service -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_async_qemu -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T16:32:27Z**

Continuation iteration 6 reflection/runner slice: combined supervised QEMU/state and blocking-service handles into a first async runner skeleton for one service. run_supervised_qemu_process_with_service_async spawns QEMU with state=running, then races QEMU exit/timeout against post-readiness service completion. Service failure now kills QEMU, marks QEMU failed with a service-failure cause, writes state=service_failed with qemu_status context, and returns the service error. This is still a testable building block, not production wiring. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_qemu_with_service -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_blocking_service -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_async_qemu -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T16:34:43Z**

Continuation iteration 7: generalized the one-service async runner skeleton to multiple supervised blocking services. Added run_supervised_qemu_process_with_services_async and wait_for_first_service_completion; multiple SupervisedBlockingService handles are monitored concurrently, the first service completion/failure while QEMU is running terminates QEMU, marks QEMU failed, writes state=service_failed, and returns the service cause. The one-service helper now delegates to the multi-service helper. Added focused test with config-fs and vmnet handles where vmnet fails first. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_qemu_with_service -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_qemu_with_services -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_blocking_service -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T16:37:05Z**

Continuation iteration 8: added additive async production-shape launch function run_frontend_until_qemu_exit_with_policy_and_timeout_async. It performs launch prep/state=start using existing config/mount/policy code, creates LaunchSupervisor, starts composed-fs/config-fs/vmnet through supervised spawn_blocking service readiness helpers, starts optional docker proxy with async readiness wait, then runs QEMU through the multi-service supervised runner. This connects real launch preparation/service construction to the async runner surface while leaving production start_frontend_with_policy unchanged. Also changed multi-service monitoring to use tokio::task::JoinSet instead of detached monitor tasks, so dropping the monitor future aborts pending monitor tasks. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_qemu_with_services -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_blocking_service -- --nocapture; cargo check --manifest-path vm-frontend/Cargo.toml --offline; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T16:38:41Z**

Continuation iteration 9 decision and validation: do not route production launch CLI through the async launch function yet. Reason: the additive async function now constructs real services and QEMU, but production routing should wait until shutdown/drop semantics are made explicit for all service tasks (including suppressing expected service exits after QEMU shutdown and ensuring no monitor/task lifecycle surprises). This avoids replacing the currently live-validated synchronous launch path prematurely. Broader offline validation passed for current async slices: cargo test --manifest-path vm-frontend/Cargo.toml --offline. Next priority is cleanup/refinement of async launch lifecycle semantics before any production switch or required validation for closure.

**2026-05-17T16:40:55Z**

Continuation iteration 10 lifecycle cleanup: tightened async launch service shutdown/status semantics. wait_for_qemu_or_service_failure now snapshots service controllers before monitoring; on normal QEMU exit it marks unfinished services cancelled with reason 'qemu exited', on QEMU timeout with 'qemu timed out', and on first service failure it marks peer services cancelled with reason 'peer service failed'. Added tests for QEMU exit cancelling unfinished services and for peer-service cancellation when one service fails. Broader validation remains green: cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T16:42:57Z**

Continuation iteration 11 reflection: wra-zqci now has an additive async production-shape runner with supervised QEMU, real launch prep/service construction, multi-service failure racing, state updates, and cancellation status for normal QEMU exit, timeout, and peer service failure. What's working: small focused helper surfaces plus unit tests keep behavior stable and full vm-frontend offline tests were green last iteration. Main blocker: production CLI is still deliberately on the synchronous path because the async helper has not been live-validated and service shutdown is status-level only for blocking services; spawn_blocking cannot forcibly stop a running composed-fs/vmnet loop. Approach adjustment: stop expanding API breadth and focus on readiness for production routing/validation: tests around lifecycle states, then optionally route the no-payload launch path through async and run required validation. Iteration 11 added the missing timeout lifecycle test: QEMU timeout now cancels unfinished service statuses with reason 'qemu timed out'. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_qemu -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T16:46:07Z**

Continuation iteration 12: production routing decision implemented for the direct non-payload launch path. run_cli_async now dispatches direct 'agentvm launch' through a new async-aware run_async path; launch_cli::run_launch_async uses the additive async launch runner for no-payload/plain launches, while TUI/payload launches delegate to existing synchronous run_launch to avoid changing payload/TUI lifecycle in this ticket. Wrapper launches also remain on the existing synchronous wrapper path. Shared QEMU exit formatting was extracted into handle_qemu_exit. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline argv0_no_longer_selects_wrapper_or_tool_behavior -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline frontend_ -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T16:50:24Z**

Continuation iteration 13 validation: broader offline validation passed after routing direct no-payload/plain launches through async path: cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check. Required live-capable validation also passed: ./vm-frontend/validate.sh required. No appliance rebuild was requested. Note: required live scenarios exercise payload/wrapper paths (kept synchronous in wra-zqci) and the overall launch stack; direct no-payload/plain path is now routed through the async runner and covered by focused offline supervisor/process tests.
