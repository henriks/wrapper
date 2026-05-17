---
id: wra-35eb
status: closed
deps: []
links: [wra-ylfx, wra-d6vo]
created: 2026-05-16T16:13:56Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-bjaa
tags: [async, tokio, payload, signals, tui]
---
# Design shared signal and resize policy for async-ready payload clients

Problem:
Plain mode installs process-wide signal handlers and writes to a self-pipe, while TUI mode maps Ctrl-C as guest input. The code needs a shared policy for forwarded signals, local abort, resize events, and shutdown escalation before or during Tokio adoption.

Grounding:
- vm-frontend/src/payload_client.rs:417-447 covers signal forwarder setup.
- vm-frontend/src/payload_client.rs:505-518 covers signal pipe/handler behavior.
- vm-frontend/src/tui.rs:590 and nearby code handle TUI control/input behavior.

Why async helps:
tokio::signal::unix can model signals as streams and combine them with payload IO cancellation. It makes first-Ctrl-C-to-guest, second-Ctrl-C-local-abort semantics straightforward.

Can be solved without Tokio:
Yes. wra-d6vo owns the immediate fix: pipe2(O_NONBLOCK | O_CLOEXEC), coalesced EAGAIN, and local cancellation semantics.

Proposed implementation shape:
Define a signal policy shared by plain and TUI: forwarded signals, local-abort signals, resize forwarding, and escalation rules. Keep guest signal/resize frames unchanged. If Tokio is later introduced, implement this policy behind the same interface instead of spreading Tokio signal APIs through payload_client.rs.

Risks:
Tokio signal handling is process-global too; dropping a signal stream may not restore default process behavior like the current sigaction restore path. Tests must avoid order dependence.

Relationships:
- Overlaps wra-d6vo; this ticket should track the runtime-compatible abstraction, while wra-d6vo remains the immediate bug fix.
- Supports wra-ylfx by giving shutdown a reliable local abort path.

Validation:
- Signal-storm regression.
- Repeated Ctrl-C escalation.
- SIGTERM during blocked payload receive.
- Resize forwarding still sends W frames.
- Include at least one PTY/TUI test for Ctrl-C mapping.


## Notes

**2026-05-16T16:29:18Z**

Added PayloadControlPolicy/PayloadControlAction in vm-frontend/src/payload_client.rs to make interactive signal, resize, and local-abort escalation rules explicit and reusable. TUI Ctrl-C mapping now consults the shared policy for the first-interrupt forward-to-guest case. Tests cover installed Unix signals, first/repeated Ctrl-C, SIGTERM local abort, SIGHUP forward, SIGWINCH resize, and disabled policy. This is policy groundwork only; cancellable local-abort execution remains for wra-m7gg/wra-d6vo.

**2026-05-16T16:31:17Z**

Extended the shared payload control policy from documentation into the plain-mode signal loop: SignalForwarder now evaluates PayloadControlPolicy actions, forwards first SIGINT, forwards SIGWINCH as resize, treats repeated SIGINT/SIGTERM as LocalAbort, sets the done flag, and shuts down the payload stream to interrupt blocking recv. Added control_action_application test plus TUI helper coverage for repeated Ctrl-C mapping. Verified with cargo fmt and focused offline cargo tests: policy, control_action_application, ctrl_c_becomes_guest_signal.

**2026-05-16T16:33:05Z**

Implemented nonblocking/CLOEXEC Unix signal self-pipe creation in vm-frontend/src/payload_client.rs and updated the signal forward loop to tolerate WouldBlock/Interrupted instead of exiting or blocking. Added tests that assert pipe flags and that payload_signal_handler returns when the nonblocking pipe is full (signal-storm safety). Verified with cargo fmt plus focused offline tests: signal_pipe, signal_handler_ignores_full_nonblocking_pipe, control_action_application.

**2026-05-16T16:34:30Z**

Added end-to-end host-side signal tests: signal_forward_loop_forwards_then_aborts_on_repeated_interrupt verifies first SIGINT writes an S frame and second SIGINT shuts down the stream/done flag; signal_forwarder_terminates_blocked_payload_receive_on_sigterm runs run_payload_tcp_with_control against a fake server that accepts the request and stalls, then injects SIGTERM through the installed handler and asserts the blocked receive terminates. Added test serialization around SIGNAL_WRITE_FD users. Verified with cargo fmt and focused offline tests for both cases.
