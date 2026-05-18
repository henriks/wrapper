---
id: wra-3s2f
status: closed
deps: []
links: [wra-8fjd]
created: 2026-05-18T10:35:20Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-emj5
tags: [cleanup, payload, tui, tokio]
---
# Collapse payload client onto one session architecture

The async supervisor launch path still hands payload execution to blocking payload sessions through spawn_blocking, while AsyncPayloadSession exists mostly for tests. This leaves a sync/async bridge in vm-frontend/src/payload_client.rs, vm-frontend/src/launch_cli.rs, and vm-frontend/src/tui.rs. Choose one production payload session architecture and delete the other; preferred direction is to make plain launch, TUI, payload-client diagnostics, and self-test use the async frame/session path directly.

## Design

Audit PayloadSessionRunner, PayloadSession, PayloadWriter, run_payload_tcp_with_control, run_diagnostic_tcp, send_frame/recv_frame, AsyncPayloadSession, launch_cli payload spawn_blocking, and tui::run_payload_viewport. Migrate callers to one outcome model for plain and TUI payloads. Do not add a third adapter. When done, either the sync TCP/session implementation is deleted or the async implementation is deleted with a written rationale; keeping both is not acceptable.

## Acceptance Criteria

Production payload launch and TUI use one session runner; unused sync or async payload session types and framing helpers are removed or test-only; TUI and plain payload failures share a structured outcome suitable for post-restore reporting; focused payload/TUI tests and required validation are recorded before close.


## Notes

**2026-05-18T10:55:02Z**

Iteration 4 audit: current production plain launch still uses tokio::task::spawn_blocking around run_payload_tcp_with_control in vm-frontend/src/launch_cli.rs, while TUI uses spawn_blocking around tui::run_payload_viewport. Both paths rely on sync PayloadSession/PayloadWriter framing in vm-frontend/src/payload_client.rs; AsyncPayloadSession is currently only exercised by tests and async diagnostic helpers. Existing PayloadSessionOutcome already represents Exit/Failure/Cancelled and can be reused as the shared structured outcome for plain/TUI and wra-8fjd post-restore reporting. Next implementation step should move plain launch first to AsyncPayloadSession on the existing runtime, then adapt TUI input/reader plumbing to AsyncPayloadCommandSender rather than keeping the sync PayloadWriter clone.

**2026-05-18T10:58:46Z**

Iteration 5 progress: moved the plain launch payload path off spawn_blocking and onto AsyncPayloadSession. Added run_payload_tcp_async_with_control/run_payload_session_async in vm-frontend/src/payload_client.rs, including async stdin forwarding, async stdout writes, structured PayloadSessionOutcome, and async Unix signal/resize forwarding through AsyncPayloadCommandSender. vm-frontend/src/launch_cli.rs now calls this async path directly for WrapperUiMode::Plain; TUI still uses the sync PayloadSession/PayloadWriter path and is the next production caller to migrate before deleting sync session code. Added focused async runner test async_payload_runner_forwards_input_output_and_exit. Also enabled Tokio io-std workspace feature for async stdin/stdout. Verification: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --lib async_payload_runner_forwards_input_output_and_exit; cargo check --manifest-path vm-frontend/Cargo.toml --lib --bin agentvm-frontend. Required validation still pending before close. No sudo appliance rebuild required for these payload/frontend changes.

**2026-05-18T11:01:33Z**

Iteration 6 progress: migrated TUI payload execution to AsyncPayloadSession/AsyncPayloadCommandSender. vm-frontend/src/tui.rs now has async run_payload_viewport(SocketAddr, ...) returning PayloadSessionOutcome, connects with tokio::net::TcpStream, receives PayloadEvent from AsyncPayloadSession, and sends guest input/resize/signal through AsyncPayloadCommandSender. vm-frontend/src/launch_cli.rs no longer uses spawn_blocking for TUI and defers PayloadSessionOutcome::into_exit_code until after filesystem flush/supervisor shutdown/launch completion, giving plain and TUI the same structured outcome path for wra-8fjd. Remaining sync payload API is still used by payload-client CLI/diagnostics and tests; next step is migrate those or make sync helpers test-only before closing. Verification: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo check --manifest-path vm-frontend/Cargo.toml --lib --bin agentvm-frontend; cargo test --manifest-path vm-frontend/Cargo.toml --bin agentvm-frontend tui::tests; cargo test --manifest-path vm-frontend/Cargo.toml --test tui_terminal. Required validation still pending before close. No sudo appliance rebuild required.

**2026-05-18T11:07:16Z**

Iteration 7 progress: moved payload-client command diagnostics/primary execution and launch payload readiness/flush further onto async payload paths. Added ping_payload_async_tcp and run_diagnostic_tcp_async(_with_deadline) in vm-frontend/src/payload_client.rs; payload-client dispatch in vm-frontend/src/cli_main.rs now routes through run_async and uses async ping/diagnostic/primary execution; flush_guest_filesystems is async and uses async diagnostic. Launch payload readiness now uses an async ping loop instead of spawn_blocking; the old sync wait_for_payload_ready is cfg(validation-self-test) for current self-test usage. Remaining production-ish sync island is validation self-test: vm-frontend/src/self_test.rs still uses ping_payload/run_payload_tcp_with_control and spawn_blocking readiness, including the published-container marker helper. Verification: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --lib async_ping_uses_protocol_frames; cargo test --manifest-path vm-frontend/Cargo.toml --lib async_diagnostic_streams_output_and_enforces_deadline; cargo test --manifest-path vm-frontend/Cargo.toml --bin agentvm-frontend flush_guest_filesystems_sends_sync_diagnostic; cargo test --manifest-path vm-frontend/Cargo.toml --bin agentvm-frontend payload_readiness_timeout_reports_last_error; cargo check --manifest-path vm-frontend/Cargo.toml --lib --bin agentvm-frontend --features validation-self-test. Required validation still pending before close. No sudo appliance rebuild required.

**2026-05-18T11:11:59Z**

Iteration 8 progress: validation self-test now uses async payload helpers for readiness, published payload ping, primary payload execution, and published-container payload monitoring. vm-frontend/src/self_test.rs calls wait_for_payload_ready_async, ping_payload_async_tcp, and run_payload_tcp_async_with_control; the published-container marker path now uses an async ReadyMarkerWriter and tokio task instead of a sync payload thread. Removed the unused sync wait_for_payload_ready wrapper from launch_cli. Remaining sync payload code appears confined to payload_client.rs compatibility/test helpers and tests; next step is to make those helpers cfg(test) or delete them, then run required validation. Verification: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo check --manifest-path vm-frontend/Cargo.toml --lib --bin agentvm-frontend --features validation-self-test; cargo test --manifest-path vm-frontend/Cargo.toml --bin agentvm-self-test --features validation-self-test parses_self_test_config_defaults_and_options; cargo test --manifest-path vm-frontend/Cargo.toml --bin agentvm-self-test --features validation-self-test self_test_payload_covers_workspace_docker_and_bind_mount; cargo check --manifest-path vm-frontend/Cargo.toml --bins. No sudo appliance rebuild required.

**2026-05-18T11:34:24Z**

Iteration 9 completion: demoted the remaining sync payload session architecture to test-only by cfg(test)-gating PayloadCancelToken, PayloadSessionRunner, PayloadSession, PayloadWriter, sync ping/run_payload/run_diagnostic helpers, sync frame helpers, and sync signal forwarder support in vm-frontend/src/payload_client.rs. Production and validation paths now use AsyncPayloadSession or async ping/diagnostic helpers. During required validation, live-smoke initially hung because async ping probes had no per-probe deadline after TCP connect; fixed by adding bounded timeouts to ping_payload_async_tcp plus async payload/diagnostic TCP connects. Verification: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo check --manifest-path vm-frontend/Cargo.toml --lib --bin agentvm-frontend --features validation-self-test; cargo test --manifest-path vm-frontend/Cargo.toml --lib async_payload_runner_forwards_input_output_and_exit; cargo test --manifest-path vm-frontend/Cargo.toml --lib payload_session_surfaces_failure_events; cargo test --manifest-path vm-frontend/Cargo.toml --bin agentvm-frontend payload_readiness_timeout_reports_last_error; ./vm-frontend/validate.sh live-smoke passed; ./vm-frontend/validate.sh required passed with 300s timeout. No sudo appliance rebuild required.
