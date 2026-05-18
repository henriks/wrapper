---
id: wra-oio0
status: closed
deps: [wra-7t63, wra-01tu, wra-d0q5]
links: [wra-rie1, wra-7t63, wra-yl7i, wra-01tu]
created: 2026-05-18T05:36:22Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-9m5h
tags: [cleanup, launch, supervisor, tokio]
---
# Delete synchronous launch path after supervisor/control callers migrate

The codebase now has both an async supervisor path and a synchronous launch path. The duplicate path keeps partial-startup cleanup, payload attach, and TUI/backend bridging logic split across vm-frontend/src/launch.rs, vm-frontend/src/launch_cli.rs, and self-test helpers. After payload viewport attach/detach and other remaining callers move onto the supervisor control protocol, delete the synchronous launch path rather than hardening it further. Known overlap: wra-rie1 currently hardens partial startup cleanup, but this cleanup ticket should supersede that direction if the old path can be removed.

## Design

Audit launch entry points and remove the old sync launch/control stack. Consolidate startup, readiness, shutdown, and cleanup under the async supervisor path. Any retained helper must be shared by the async path directly, not reachable only through a fake CLI or compatibility layer. Keep tests focused on the single surviving path.

## Acceptance Criteria

No production caller uses the old synchronous launch path; obsolete partial-startup cleanup code is removed or reduced to shared async-supervisor cleanup; TUI and payload attach/detach use supervisor control; tests and validation scripts target one launch lifecycle; required validation is recorded before close.

## Notes

**2026-05-18T06:55:37Z**

Dependency update: wra-d0q5 is closed. The frontend live self-test harness now runs through validation-only bin agentvm-self-test behind the validation-self-test feature; production agentvm-frontend no longer exposes self-test dispatch, reducing one blocker for deleting the old synchronous launch path.

**2026-05-18T07:08:09Z**

wra-01tu is closed: wrapper no longer calls launch via string argv roundtrip. Wrapper now passes FrontendLaunchRequest directly to run_launch_request. Remaining blocker is wra-7t63 (payload viewport/control protocol migration) before deleting synchronous launch path.

**2026-05-18T07:30:06Z**

wra-7t63 has started. Audit confirms the blocker remains: run_launch_request_async delegates to the synchronous launch path for any payload or TUI launch; that synchronous branch owns frontend lifecycle, payload TCP attach, TUI/plain payload session, flush, and VM termination. Deleting the sync launch path should wait until payload launches use the async supervisor/control-owned payload session boundary.

**2026-05-18T07:35:33Z**

wra-7t63 iteration 13 made the first control-boundary migration step: supervisor status now publishes the planned payload_control_endpoint from the effective vmnet policy, and the async launch supervisor plan now carries that effective policy. This helps future TUI/plain payload attach discover endpoint state from supervisor control, but wra-oio0 remains blocked until actual payload session I/O is migrated off the synchronous launch branch.

**2026-05-18T07:40:33Z**

wra-7t63 iteration 14 moved the plain async launch payload path to supervisor-control endpoint discovery and control-socket shutdown. The synchronous launch branch is still required for TUI payload viewport attach/detach, so wra-oio0 remains blocked until TUI also uses the supervisor-owned payload session/client path.

**2026-05-18T07:45:19Z**

wra-7t63 iteration 15 removed the TUI payload fallback from run_launch_request_async and moved production agentvm argv0 wrapper dispatch to run_wrapper_async. The remaining synchronous launch path is now concentrated in the old sync run_cli/run_wrapper/run_launch_request boundary and should be the direct deletion target once wra-7t63 is validated/closed.

**2026-05-18T07:52:02Z**

wra-7t63 iteration 16 required validation passed after hardening async payload launch failure handling. The remaining sync launch deletion can rely on required validation evidence for production wrapper/TUI payload migration, but wra-7t63 still needs a final closure decision/documentation of attach/detach/reconnect semantics.

**2026-05-18T07:54:11Z**

Iteration 17 audit after starting ticket: wra-7t63 is closed and required validation passed, so blocker is cleared. Remaining synchronous launch ownership is concentrated in vm-frontend/src/launch.rs (run_frontend_until_qemu_exit_with_policy_and_timeout, RunningFrontend, start_frontend_with_policy), vm-frontend/src/launch_cli.rs (run_launch/run_launch_request sync branch), vm-frontend/src/wrapper.rs (sync run_wrapper used only by legacy sync run_cli/test boundary), and validation-only vm-frontend/src/self_test.rs (still starts start_frontend_with_policy and terminates RunningFrontend directly). Production async dispatch no longer needs the sync path, but wra-oio0 should migrate agentvm-self-test to async supervisor/control shutdown before deleting RunningFrontend/start_frontend_with_policy.

**2026-05-18T07:57:04Z**

Iteration 18 progress: migrated validation-only agentvm-self-test from start_frontend_with_policy/RunningFrontend::terminate to the async supervisor path. run_self_test is now async, starts run_frontend_until_qemu_exit_with_policy_and_timeout_async, waits for payload readiness while racing the launch task for early failures, then flushes and requests shutdown through SupervisorControlClient. Remaining start_frontend_with_policy/RunningFrontend callers are now the obsolete sync launch helpers/tests, not self-test. Targeted validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline --features validation-self-test parses_self_test_config_defaults_and_options -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features --bins -- --nocapture; git diff --check.

**2026-05-18T08:04:26Z**

Iteration 19 progress: deleted the obsolete synchronous launch implementation. Removed RunningFrontend, start_frontend_with_policy, sync run_frontend_until_qemu_exit* wrappers, sync wait_for_qemu/wait_for_path, sync launch_cli run_launch/run_launch_request branch, sync wrapper run_wrapper, and RunningFrontend-specific unit tests. Legacy sync run_cli now delegates launch/wrapper invocations through a Tokio runtime to the async dispatch for test-only compatibility. rg no longer finds RunningFrontend/start_frontend_with_policy/run_launch/run_launch_request/run_wrapper in vm-frontend/src or tests. Targeted validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features --lib launch::tests:: -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features --bins -- --nocapture (log /tmp/iter19-bins.log); git diff --check.
