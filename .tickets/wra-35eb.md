---
id: wra-35eb
status: open
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

