---
id: wra-yl7i
status: in_progress
deps: [wra-n0fe, wra-zqci, wra-cvmy]
links: [wra-bjaa, wra-zqci, wra-jkeg, wra-xcvq, wra-n0fe]
created: 2026-05-16T16:12:05Z
type: feature
priority: 1
assignee: Henrik Saksela
tags: [tui, control-socket, frontend, architecture, agentvm]
---
# Make the TUI a control-socket frontend to the agentvm supervisor

Problem / direction:
The current TUI is coupled directly into the launch/payload path. It should become a frontend client to an actual running agentvm supervisor process, communicating over a control socket or equivalent local IPC. The supervisor should own VM lifecycle, payload sessions, diagnostics, status, logs, and shutdown semantics; the TUI should render/control that state rather than being the process that directly runs the VM path.

Why this matters:
- Makes future TUI features easier to develop: attach/detach, richer status panes, diagnostics, multiple views, prompts/decisions, and reconnect behavior.
- Separates durable runtime ownership from presentation. Closing or crashing the TUI should not necessarily imply ambiguous VM/runtime cleanup unless explicitly requested.
- Aligns with the async service-IO direction: agentvm can expose a structured control plane while internal service IO evolves independently.
- Provides a cleaner boundary for plain CLI mode, TUI mode, tests, and possible future non-TUI clients.

Relevant current code:
- vm-frontend/src/main.rs run_launch currently decides plain vs TUI payload mode and directly starts/terminates the frontend.
- vm-frontend/src/tui.rs currently runs the payload viewport in-process.
- vm-frontend/src/payload_client.rs contains the payload protocol used after launch readiness.
- vm-frontend/src/launch.rs owns RunningFrontend lifecycle, QEMU process handling, state.json, and termination.
- Runtime artifacts already include .sandbox/docker-vm/run paths that could host a local control socket.

Design questions to answer:
- Is the control socket Unix-domain only, and where is it located under RuntimePaths?
- What is the minimum protocol surface: status snapshot, subscribe events/logs, start payload, resize/input/signal payload, diagnostics, graceful shutdown, force shutdown?
- How does auth/safety work for a project-local socket?
- Can plain CLI and TUI both use the same control client API?
- What happens on TUI disconnect while a payload is running?
- How are protocol compatibility and config-file compatibility documented/tested?

Suggested implementation shape:
1. Define a small typed control protocol and supervisor/client boundary without changing runtime behavior.
2. Move launch ownership into an agentvm supervisor that exposes the socket and writes state/events.
3. Rework plain payload mode and TUI mode to use the same control client.
4. Add attach/reconnect semantics after the basic protocol is stable.

Validation:
- Unit tests for protocol framing/serialization and state transitions.
- Integration test that starts a supervisor, connects a client, runs a payload, disconnects/reconnects, and observes consistent status.
- TUI/PTY test proving terminal UI is only a client and nonzero payload failures are still visible after terminal restore.
- Live validation before closing because this changes lifecycle ownership.

Related work:
- Link to async runtime epic wra-bjaa: the control plane should be designed so vmnet/service IO can move to Tokio without changing TUI semantics.
- Related stability tickets include payload cancellation, graceful shutdown, and TUI post-restore summaries.


## Notes

**2026-05-17T10:20:56Z**

Linked into refactoring option 1 epic wra-xcvq. Dependencies added on option-1 supervisor skeleton (wra-n0fe), async launch supervision (wra-zqci), and shared payload protocol (wra-cvmy), because the TUI control-socket frontend should build on those backend boundaries rather than direct launch/payload coupling.

**2026-05-17T20:58:09Z**

Starting after wra-0a0r closed. First slice will be protocol/boundary-only: define a small typed supervisor control protocol, deterministic project-local Unix socket path under RuntimePaths/run_dir, snapshot helpers over existing LaunchSupervisor state, and parser/serializer/fuzz coverage. Do not yet change launch ownership or TUI runtime behavior; keep current lifecycle stable until the control API has focused tests.

**2026-05-17T21:00:16Z**

First control-plane seam implemented without changing launch/TUI runtime behavior. Added vm-frontend/src/supervisor_control.rs with a versioned JSON supervisor-control envelope, request/response types for status snapshot/subscription/shutdown, snapshot DTO over existing LaunchSupervisor state, and deterministic control_socket_path under RuntimePaths.run_dir as agentvm-control.sock. Added serde derives to existing supervisor task/shutdown DTOs plus LaunchSupervisor::current_shutdown. Added focused tests and fuzz target coverage for arbitrary control message parsing. Validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervisor_control -- --nocapture; cargo test --manifest-path vm-frontend/fuzz/Cargo.toml --offline --no-run --bin supervisor_control_message.

**2026-05-17T21:03:52Z**

Second control-plane seam: added bounded one-request Unix-socket IPC helpers in supervisor_control. New pieces: MAX_CONTROL_MESSAGE_BYTES, SupervisorControlIoError, bind_control_socket, control_client_request, serve_control_listener_once, handle_control_connection, read/write bounded JSON envelope helpers, and apply_control_request for status snapshot and shutdown ack. Malformed/oversized requests return typed Error responses instead of panicking; SubscribeStatus remains an explicit not-implemented response for a later streaming slice. Added Unix-socket client/server tests for status snapshot and shutdown plus malformed/oversized request tests. Validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervisor_control -- --nocapture; cargo test --manifest-path vm-frontend/fuzz/Cargo.toml --offline --no-run --bin supervisor_control_message.

**2026-05-17T21:06:38Z**

Third control-plane seam: added SupervisorControlClient as a reusable frontend/TUI adapter with status_snapshot() and request_shutdown() methods, plus serve_control_listener_until_shutdown() for a sidecar server loop that accepts multiple local clients until the existing LaunchSupervisor shutdown watch fires. The loop spawns per-connection handlers so one slow client does not block later accepts. Added focused test proving the client adapter reads QEMU starting status over a Unix socket, requests shutdown, and the server loop exits. Launch/TUI ownership is still unchanged; this avoids exposing a live shutdown command before launch cancellation semantics are wired. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervisor_control -- --nocapture; cargo test --manifest-path vm-frontend/fuzz/Cargo.toml --offline --no-run --bin supervisor_control_message.
