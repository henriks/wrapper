# Continue wra-xcvq: Option 1 Tokio-boundary architecture refactor

Continue the same task tracked in `.ralph/wra-xcvq-option1.md` for 20 additional iterations.

## Goal
Progress `tk` epic `wra-xcvq`: make agentvm Tokio-oriented at orchestration and byte-stream I/O boundaries while keeping protocol/state cores synchronous and composed-fs/vhost filesystem execution bounded blocking.

## Current state
- Completed and closed after required live-capable validation: `wra-9glk`, `wra-09v9`, `wra-r070`, `wra-rjt4`, `wra-n0fe`, `wra-cvmy`, `wra-jkeg`.
- Current in-progress ticket: `wra-zqci` async launch process/readiness supervision.
- Last completed slice: added additive Tokio QEMU/process helpers in `vm-frontend/src/launch.rs` (`run_qemu_process_async`, `spawn_qemu_process_async`, `wait_for_qemu_async`) and tests; focused validation passed. Production `start_frontend_with_policy` remains synchronous.

## Ground rules
- Use `tk` for ticket state; add notes for significant findings.
- Process about 2 checklist items per iteration.
- Update `.ralph/wra-xcvq-option1.md` each iteration.
- Required validation before closing tickets: `./vm-frontend/validate.sh required`.
- If validation says the appliance must be rebuilt, tell the user exactly what needs rebuilding and why, then switch to other useful epic work until rebuild is confirmed.
- Do not close tickets/epic without live-capable required validation.
- Preserve config-file compatibility; update `vm-frontend/config-json.md` and tests if config semantics change.
- Prefer small behavior-preserving/additive slices.

## Next priorities
1. Continue `wra-zqci`: wire async process helpers into a supervised async launch runner and service readiness/failure ownership.
2. Keep composed-fs/config-fs execution bounded blocking; do not move FS request execution onto Tokio core workers.
3. Add focused tests for async launch supervision timeout, service failure propagation, readiness, and state updates.
4. Run broader validation when slices stabilize; required validation before closure.

## Progress
- [x] Iteration 1: wire async QEMU process helpers into supervisor task status ownership.
  - Added `run_supervised_qemu_process_async` in `vm-frontend/src/launch.rs`.
  - Marks QEMU task starting/ready/finished/failed around async spawn/wait/timeout while preserving `QemuExit` return semantics.
  - Added focused async tests for supervised QEMU normal exit and timeout status publication.
  - Focused validation passed: `cargo fmt --manifest-path vm-frontend/Cargo.toml`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_async_qemu -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline async_qemu -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline launch_ -- --nocapture`; `cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check`.
- [x] Iteration 2: add async service readiness helper with supervisor status publication.
  - Added `wait_for_path_async` and `wait_for_service_ready_async` in `vm-frontend/src/launch.rs`.
  - The async path helper mirrors synchronous `wait_for_path` with Tokio sleep/timeout boundaries and shared timeout message construction.
  - The service readiness helper marks service controllers ready or failed on timeout.
  - Focused validation passed: `cargo fmt --manifest-path vm-frontend/Cargo.toml`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline async_service_readiness -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline async_qemu -- --nocapture`; `cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check`.
- [x] Iteration 3: add state-writing supervised async QEMU runner surface.
  - Added `run_supervised_qemu_process_with_state_async`, which writes `state.json` as `running` after async spawn and `exited`/`timed_out` after wait.
  - Refactored shared supervised QEMU wait/status handling into `finish_supervised_qemu_wait`.
  - Added focused tests for `state.json` updates on supervised async QEMU exit and timeout.
  - Focused validation passed: `cargo fmt --manifest-path vm-frontend/Cargo.toml`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_async_qemu -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline async_service_readiness -- --nocapture`; `cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check`.
- [x] Iteration 4: add supervised blocking service readiness helper for bounded-blocking service startup.
  - Added `spawn_supervised_blocking_service_until_ready`, which runs blocking service startup with `tokio::task::spawn_blocking`, races readiness against early service exit, returns the join handle after readiness, and marks failed on readiness timeout or early service failure.
  - Added focused tests for ready service handle return and early service failure status publication.
  - Focused validation passed: `cargo fmt --manifest-path vm-frontend/Cargo.toml`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_blocking_service -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline async_service_readiness -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_async_qemu -- --nocapture`; `cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check`.
- [x] Iteration 5: add service handle wrapper for post-readiness blocking service failure observation.
  - `spawn_supervised_blocking_service_until_ready` now returns `SupervisedBlockingService`, retaining the task controller and join handle.
  - `SupervisedBlockingService::wait` marks services finished on clean exit and failed on late service error or join failure.
  - Added a focused test for late service failure after readiness.
  - Focused validation passed: `cargo fmt --manifest-path vm-frontend/Cargo.toml`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_blocking_service -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_async_qemu -- --nocapture`; `cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check`.
- [x] Iteration 6: add first async QEMU-plus-service runner skeleton with service-failure ownership.
  - Added `run_supervised_qemu_process_with_service_async`, which writes QEMU `running` state, races QEMU exit/timeout against a supervised service handle, kills QEMU on service failure, marks QEMU failed, writes `state.json` as `service_failed`, and returns the service error.
  - Added focused test for service failure while QEMU is running.
  - Focused validation passed: `cargo fmt --manifest-path vm-frontend/Cargo.toml`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_qemu_with_service -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_blocking_service -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_async_qemu -- --nocapture`; `cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check`.
- [x] Iteration 7: generalize async QEMU/service runner skeleton from one service to multiple supervised service handles.
  - Added `run_supervised_qemu_process_with_services_async` and `wait_for_first_service_completion`.
  - The one-service runner now delegates to the multi-service helper.
  - Added focused test with config-fs and vmnet handles where the first service failure stops QEMU and writes `service_failed` state.
  - Focused validation passed: `cargo fmt --manifest-path vm-frontend/Cargo.toml`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_qemu_with_service -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_qemu_with_services -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_blocking_service -- --nocapture`; `cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check`.
- [x] Iteration 8: connect the multi-service runner to production launch preparation/service construction behind an additive async launch function.
  - Added `run_frontend_until_qemu_exit_with_policy_and_timeout_async`, which performs launch prep/state, starts composed-fs/config-fs/vmnet through supervised bounded-blocking service helpers, waits for optional docker proxy readiness asynchronously, and runs QEMU through the multi-service supervised runner.
  - Changed multi-service monitoring to use `tokio::task::JoinSet` instead of detached monitor tasks.
  - Production `start_frontend_with_policy` remains unchanged.
  - Focused validation passed: `cargo fmt --manifest-path vm-frontend/Cargo.toml`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_qemu_with_services -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_blocking_service -- --nocapture`; `cargo check --manifest-path vm-frontend/Cargo.toml --offline`; `cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check`.
- [x] Iteration 9: decide production routing and run broader validation.
  - Decision: do not route production launch CLI through the async launch function yet; keep the live-validated synchronous launch path until async shutdown/drop semantics for all services are explicit.
  - Broader validation passed: `cargo test --manifest-path vm-frontend/Cargo.toml --offline`.
- [x] Iteration 10: tighten async launch lifecycle shutdown/status semantics before any production switch.
  - `wait_for_qemu_or_service_failure` now snapshots service controllers before monitoring.
  - On normal QEMU exit it marks unfinished services cancelled with reason `qemu exited`; on QEMU timeout with `qemu timed out`; and on first service failure it marks peer services cancelled with reason `peer service failed`.
  - Added focused tests for QEMU-exit cancellation and peer-service cancellation on first service failure.
  - Validation passed: `cargo test --manifest-path vm-frontend/Cargo.toml --offline`; `cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check`.
- [x] Iteration 11: reflection checkpoint and missing timeout lifecycle test.
  - Reflection: additive async launch pieces are now broad enough; next work should focus on production-routing readiness rather than adding more standalone helpers.
  - Added focused coverage that QEMU timeout cancels unfinished service statuses with reason `qemu timed out`.
  - Focused validation passed: `cargo fmt --manifest-path vm-frontend/Cargo.toml`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_qemu -- --nocapture`; `cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check`.
- [x] Iteration 12: route direct no-payload/plain launch through the async runner.
  - `run_cli_async` now dispatches direct `agentvm launch` through async-aware `run_async`.
  - Added `launch_cli::run_launch_async`, which uses the async launch runner for no-payload/plain launches and delegates TUI/payload launches back to existing synchronous `run_launch`.
  - Wrapper launches remain on the existing synchronous wrapper path.
  - Extracted shared QEMU exit formatting into `handle_qemu_exit`.
  - Focused validation passed: `cargo fmt --manifest-path vm-frontend/Cargo.toml`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline argv0_no_longer_selects_wrapper_or_tool_behavior -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline frontend_ -- --nocapture`; `cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check`.
- [x] Iteration 13: validate and close `wra-zqci`, then start `wra-662v`.
  - Broader offline validation passed: `cargo test --manifest-path vm-frontend/Cargo.toml --offline`; `cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check`.
  - Required live-capable validation passed: `./vm-frontend/validate.sh required`; no appliance rebuild was requested.
  - Closed `wra-zqci`.
  - Started `wra-662v` and added a scope note: implement opt-in Rust/Tokio guest payload service only; leave Python as default until `wra-y335` parity/live validation.
  - Reconnaissance: `guest-service/src/main.rs` is still a skeleton, `guest-service/src/lib.rs` re-exports `agentvm_payload_protocol`, and Python `docker/guest-payload-server.py` is the behavior baseline for TCP serving, ping, primary payload sessions, bounded diagnostics, PTY/process groups, stdin/signal/resize, timeouts, caps, and failure frames.
- [x] Iteration 14: plan and implement the smallest tested `wra-662v` slice.
  - Chose a protocol/session-admission core before process execution.
  - Added `GuestServiceLimits`/`GuestServiceState` in `guest-service/src/lib.rs` with Python-baseline defaults.
  - Added bounded client, single-primary, and diagnostic admission guards.
  - Added async initial-frame handling over `AsyncRead`/`AsyncWrite`: ping OK, protocol/oversized failures attempt failure frames, unexpected initial frames fail, and valid primary/diagnostic requests are parsed/admitted but return explicit not-implemented failures.
  - Added focused Tokio duplex/admission tests for ping, unexpected initial frames, primary guard behavior, diagnostic limit behavior, and busy primary/diagnostic failure frames.
  - Focused validation passed: `cargo fmt --manifest-path guest-service/Cargo.toml`; `cargo test --manifest-path guest-service/Cargo.toml --offline -- --nocapture`; `cargo fmt --manifest-path guest-service/Cargo.toml -- --check`.
- [x] Iteration 15: add real diagnostic command execution with timeout/output caps.
  - Diagnostic requests now spawn `/bin/sh -c` with stdin closed and stdout/stderr piped.
  - The Rust service streams bounded `OUTPUT` frames and sends diagnostic `EXIT` JSON compatible with the frontend/Python schema (`exit_code`, `diagnostic`, `timed_out`, `truncated`).
  - Diagnostic timeout is capped to 60s and output to 4MiB; timeout/truncation kills the child and reports exit 124/125 respectively.
  - Added focused tests for stdout/stderr plus exit code, output-limit truncation, and timeout.
  - Focused validation passed: `cargo fmt --manifest-path guest-service/Cargo.toml`; `cargo test --manifest-path guest-service/Cargo.toml --offline -- --nocapture`; `cargo fmt --manifest-path guest-service/Cargo.toml -- --check`.
- [x] Iteration 16: reflection plus TCP listener/CLI serving path.
  - Reflection: `wra-662v` is working best as small opt-in guest-service layers; shared protocol plus Tokio duplex/listener tests are giving fast feedback. Primary payload parity remains the biggest risk because it needs PTY, controlling-terminal, process-group, stdin/signal/resize, slow-client, and UID/GID/HOME behavior.
  - Added `serve_tcp`/`serve_listener` around `tokio::net::TcpListener` with per-client task spawning.
  - Added typed TCP bind/accept errors.
  - Replaced the skeleton binary with a Tokio CLI accepting Python-compatible serving flags: `--tcp-port`, `--tcp-host`, `--max-clients`, `--max-diagnostics`, `--initial-timeout`, `--io-timeout`.
  - Added CLI parser tests and a loopback listener ping test.
  - Focused validation passed: `cargo fmt --manifest-path guest-service/Cargo.toml`; `cargo test --manifest-path guest-service/Cargo.toml --offline -- --nocapture`; `cargo fmt --manifest-path guest-service/Cargo.toml -- --check`.
- [x] Iteration 17: choose and add opt-in appliance wiring before primary PTY/control handling.
  - Decision: wire the reachable diagnostic-capable Rust service into the appliance opt-in path first, because that gives future parity/live validation real plumbing while keeping Python default unchanged.
  - `docker/guest-init.sh` now keeps Python as default and supports `agentvm_payload_service=rust`; rust mode requires executable `/usr/local/libexec/agentvm-guest-service` and fails closed if absent.
  - `docker/build-appliance.sh` now supports `AGENTVM_PAYLOAD_SERVICE=rust`, requires `AGENTVM_GUEST_SERVICE_BIN`, and adds `agentvm_payload_service=rust` to the manifest kernel cmdline only for that opt-in.
  - Added source-only shell tests for guest-init service selection and build-appliance opt-in/kernel-cmdline behavior.
  - Focused validation passed: `sh docker/tests/test_guest_init.sh`; `bash docker/tests/test_build_appliance.sh`; `cargo fmt --manifest-path guest-service/Cargo.toml -- --check`; `cargo test --manifest-path guest-service/Cargo.toml --offline -- --nocapture`.
  - Caveat: appliance source scripts changed, so required validation/closure will likely require rebuilding appliance artifacts with updated source hashes.
- [x] Iteration 18: start primary payload execution parity.
  - Added an initial non-PTY Tokio primary runner: `/bin/sh -c` with piped stdin/stdout/stderr.
  - Streams stdout/stderr as `OUTPUT` frames through a bounded channel and sends normal `EXIT` JSON.
  - Forwards `INPUT` frames into child stdin; parses `SIGNAL`/`RESIZE`; sends signals to the child PID; terminates on client disconnect/output write failure.
  - Added focused tests for primary stdout/stderr plus exit code and stdin forwarding.
  - Caveat: full Python parity still requires PTY, controlling-terminal/session/process-group behavior, UID/GID/HOME setup, real resize handling, and slow-writer cleanup.
  - Focused validation passed: `cargo fmt --manifest-path guest-service/Cargo.toml`; `cargo test --manifest-path guest-service/Cargo.toml --offline -- --nocapture`; `cargo fmt --manifest-path guest-service/Cargo.toml -- --check`.
- [x] Iteration 19: continue primary parity with process-group/session and disconnect cleanup.
  - Primary and diagnostic child commands now run in a new session via `setsid()` pre-exec.
  - Primary signal forwarding and disconnect/write-failure cleanup now target the child process group rather than only the shell PID.
  - Diagnostic timeout/truncation now SIGKILLs the child process group before the child fallback kill.
  - The primary control loop disables further control reads after EOF/error, avoiding repeated EOF spin while waiting for child termination.
  - Added focused tests for process-group signal forwarding and client-disconnect cleanup of a long-running primary payload.
  - Remaining parity gaps: PTY allocation/controlling terminal, real resize ioctl handling, UID/GID/HOME setup, graceful TERM-then-KILL escalation for primary processes that ignore TERM, and slow-writer deadlines/backpressure.
  - Focused validation passed: `cargo fmt --manifest-path guest-service/Cargo.toml`; `cargo test --manifest-path guest-service/Cargo.toml --offline guest_service_primary -- --nocapture`; `cargo test --manifest-path guest-service/Cargo.toml --offline -- --nocapture`; `cargo fmt --manifest-path guest-service/Cargo.toml -- --check`.
- [x] Iteration 20: broad validation and required-validation attempts.
  - Broad offline validation passed: `sh docker/tests/test_guest_init.sh`; `bash docker/tests/test_build_appliance.sh`; `cargo test --workspace --offline`; `cargo fmt --all -- --check`.
  - Required validation attempt #1 failed in vm-frontend offline tests due flaky `launch::tests::supervised_blocking_service_marks_failed_after_readiness`.
  - Created bug `wra-kolb` and fixed the test race by keeping the fake late-failing service alive briefly after readiness. Focused validation passed for the fixed test and the `supervised_blocking_service` group.
  - Required validation attempt #2 timed out after reporting `payload_client::tests::payload_session_runner_cancel_token_interrupts_blocked_receive` running for over 60 seconds; focused rerun of that test passed immediately.
  - Created bug `wra-cqjc` to investigate the intermittent required-validation timeout/hang.
  - Required validation did not complete and did not reach any appliance rebuild request. `wra-662v` remains in progress and must not be closed.
- [ ] Next: resolve/triage `wra-cqjc`, rerun required validation, and only then decide whether `wra-662v` can close or needs more parity work.

## Reflection
- Iteration 16: The approach should stay layered: make the Rust binary reachable/testable, then add parity-critical primary payload behavior. Do not change the default Python appliance path in `wra-662v`; that remains gated by `wra-y335`.
- Iteration 15: diagnostics are now a useful incremental proving ground for Tokio process/timeout/output-limit behavior. The remaining `wra-662v` path should choose between making the Rust service reachable via its binary TCP listener (opt-in packaging surface) and implementing primary PTY/control-channel execution; either way Python must stay default until `wra-y335`.
- Iteration 14: `wra-662v` now has a tested Rust/Tokio service core without switching defaults. The next slice should probably implement diagnostic execution first, because it exercises Tokio child process, timeout, output limits, and exit frames without the primary payload PTY/control-channel complexity.
- Iteration 13: `wra-zqci` is now closed after required validation. For `wra-662v`, avoid copying the entire Python service in one pass; first extract a small Rust service core with focused tests around protocol/session admission, then layer process/PTY and diagnostics while preserving opt-in status.
- Iteration 11: `wra-zqci` now has an additive async production-shape runner with supervised QEMU, real launch prep/service construction, multi-service failure racing, state updates, and cancellation status for normal QEMU exit, timeout, and peer service failure. Small focused helper surfaces plus unit tests are working well. Main blocker: production CLI is still deliberately on the synchronous path because the async helper has not been live-validated and service shutdown is status-level only for blocking services; `spawn_blocking` cannot forcibly stop a running composed-fs/vmnet loop. Approach adjustment: stop expanding API breadth and focus on production-routing readiness/validation.
- Iteration 6: the additive approach remains effective: supervised async process, readiness, state-writing, blocking service handles, and now one-service QEMU/service failure racing all have focused tests. The next adjustment should be to consolidate these helpers into a small multi-service runner API rather than adding more one-off helpers, then decide when to wire production launch.
- Iteration 5: `wra-zqci` is progressing through additive, focused async seams without changing production launch behavior. The main remaining gap is combining these pieces into one async launch runner that starts services, observes post-readiness service failures while QEMU runs, terminates QEMU on service failure, and records shutdown cause in state.