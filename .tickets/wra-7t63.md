---
id: wra-7t63
status: closed
deps: [wra-yl7i]
links: [wra-yl7i, wra-xcvq, wra-oio0]
created: 2026-05-17T21:24:40Z
type: feature
priority: 2
assignee: Henrik Saksela
tags: [tui, control-socket, payload, architecture, agentvm]
---
# Move payload viewport attach/detach onto supervisor control protocol

Follow-up split from wra-yl7i. The current option-1 control-plane work has established a typed bounded supervisor-control socket, status snapshot/subscription, shutdown requests wired into async QEMU/service cancellation, an async-launch sidecar, agentvm control status/shutdown, and TUI status-summary seams. The remaining larger lifecycle/UI task is to move payload viewport I/O itself behind the supervisor/control boundary so the TUI can attach/detach/reconnect without directly owning the VM launch path. Relevant code: vm-frontend/src/tui.rs run_payload_viewport, vm-frontend/src/launch_cli.rs run_launch payload/TUI branch, vm-frontend/src/payload_client.rs PayloadSession/PayloadWriter, vm-frontend/src/supervisor_control.rs control protocol/client, vm-frontend/src/launch.rs async launch supervisor sidecar. Keep the supervisor as the single owner of VM lifecycle; do not introduce a second VM owner in the TUI. Preserve .sandbox/config.json compatibility.

## Acceptance Criteria

Payload start/attach/resize/input/signal/exit semantics are exposed through the supervisor/control boundary or an explicitly documented supervisor-owned payload session boundary; TUI mode uses that client boundary instead of directly starting/stopping the frontend; disconnect/reconnect behavior is defined and tested; nonzero payload failures remain visible after terminal restore; plain CLI and TUI share the client path where practical; arbitrary control/payload input remains bounded/fuzz-covered; ./vm-frontend/validate.sh required passes before closing.


## Notes

**2026-05-17T21:24:52Z**

Creation note correction: the omitted command in the description is agentvm control status|shutdown. This ticket intentionally depends on wra-yl7i: finish the supervisor-control boundary first, then move payload viewport attach/detach/reconnect semantics behind that boundary.

**2026-05-17T21:29:46Z**

Scope clarification: this is a post-option-1 follow-up split from wra-yl7i, not a blocker for closing the wra-xcvq option-1 epic. The option-1 epic owns establishing the supervisor-control boundary and TUI observation seam; this ticket owns the later larger payload attach/detach/reconnect lifecycle rewrite.

**2026-05-18T05:39:35Z**

Cleanup epic wra-9m5h links this as a prerequisite for wra-oio0. Implement payload viewport attach/detach/reconnect as the migration needed to delete the old sync launch path, not as another adapter that leaves both launch paths alive.

**2026-05-18T07:16:47Z**

Cleanup epic wra-9m5h is now blocked from closing wra-oio0 only by this ticket. Once payload viewport attach/detach is on supervisor control protocol, wra-oio0 can delete the remaining synchronous launch path instead of maintaining parallel launch runners.

**2026-05-18T07:29:52Z**

Started audit from cleanup iteration 12. Current state: SupervisorControlRequest only supports StatusSnapshot, SubscribeStatus, and RequestShutdown; LaunchSupervisor task model has composed-fs/config-fs/vmnet/docker-proxy/qemu only; run_launch_request_async explicitly falls back to synchronous run_launch_request whenever payload args exist or TUI mode is selected. That synchronous branch owns start_frontend_with_policy, waits for payload TCP readiness, then either calls tui::run_payload_viewport directly or run_payload_tcp_with_control, flushes, and terminates the VM. This is exactly the blocker for wra-oio0: payload/TUI still couples frontend lifecycle, payload attach, and terminal viewport in one sync path. Implementation direction should be to make the async supervisor path own VM lifecycle for payload launches and expose a supervisor-owned payload session/client boundary; avoid adding an adapter that leaves the sync launch path as another production path.

**2026-05-18T07:34:09Z**

Iteration 13 progress: added the first supervisor-control payload session discovery seam. SupervisorControlSnapshot now carries an optional payload_control_endpoint derived from the supervisor-owned vmnet policy, and SupervisorControlClient exposes payload_control_endpoint() so future TUI/plain payload clients can discover the attach endpoint through the control boundary instead of launch-local state. Also fixed the async launch supervisor plan to store the effective VmnetPolicy before constructing LaunchSupervisor; previously the supervisor plan kept the default policy even though the running vmnet service was launched with the computed policy. Targeted validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features supervisor_control -- --nocapture.

**2026-05-18T07:35:15Z**

Additional iteration 13 hardening: extracted supervisor_plan_with_policy for the async launch supervisor setup and covered it with launch::tests::supervisor_plan_with_policy_carries_effective_vmnet_policy, ensuring future payload launches publish the effective payload listener in supervisor state rather than the default vmnet policy. Targeted validation also passed for that test; git diff --check passed after formatting.

**2026-05-18T07:40:33Z**

Iteration 14 progress: migrated plain (non-TUI) payload launch off the synchronous frontend owner when invoked through run_launch_request_async. Plain payload launch now starts the async supervisor-owned frontend, discovers the payload listener via SupervisorControlClient::payload_control_endpoint(), waits for guest payload readiness, runs the existing payload client, flushes guest filesystems, and requests supervisor shutdown over the control socket instead of terminating a RunningFrontend directly. TUI still falls back to the sync path, so this does not unblock wra-oio0 yet. Targeted validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features --bins run_launch -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features supervisor_control -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features supervisor_plan_with_policy_carries_effective_vmnet_policy -- --nocapture; git diff --check.

**2026-05-18T07:45:19Z**

Iteration 15 progress: migrated TUI payload launches that go through production wrapper/async launch dispatch onto the same supervisor-owned payload path as plain mode. run_launch_request_async no longer falls back to synchronous run_launch_request for TUI payloads; it runs tui::run_payload_viewport against the endpoint discovered from SupervisorControlClient::payload_control_endpoint(), then flushes and requests supervisor shutdown over the control socket. agentvm argv0 wrapper dispatch now calls run_wrapper_async, while the old sync run_wrapper/run_launch path remains only for the legacy sync run_cli/test boundary and for wra-oio0 to delete. Targeted validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features --bins -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features supervisor_control -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features supervisor_plan_with_policy_carries_effective_vmnet_policy -- --nocapture; cargo fmt; git diff --check.

**2026-05-18T07:52:02Z**

Iteration 16 reflection/validation: ran ./vm-frontend/validate.sh required for the supervisor-control payload migration. First run failed in tests/tui_terminal.rs configured_startup_terminal_skips_dialog_and_reports_launch_artifacts because async payload launch could wait for payload readiness for 120s after an early frontend/QEMU failure, so the failure artifact diagnostic was not emitted before the test timeout. Fixed by racing payload endpoint discovery/readiness against the async launch task and by failing early for missing absolute QEMU paths in launch input validation. Targeted tui_terminal regression passed, then ./vm-frontend/validate.sh required passed including live-smoke and live-setup-tools. Passing log: /tmp/pi-bash-5f61aeae608bdd10.log.

**2026-05-18T07:52:35Z**

Current attach/detach/reconnect semantics after iteration 16: attach is a single primary payload TCP session whose endpoint is discovered from supervisor state; plain and TUI clients share that supervisor-owned endpoint path. Client/TUI disconnect is not a detach-preserve operation: the payload path flushes guest filesystems and requests supervisor shutdown after payload completion/error. Reconnect is supported for supervisor status/control clients, but not for reattaching to an existing primary payload stream because the guest payload protocol binds primary payload I/O to one TCP session. If persistent payload reconnect is required beyond the cleanup deletion path, split it into a separate FOLLOW-UP rather than keeping the synchronous launch owner alive.
