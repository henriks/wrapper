---
id: wra-m7gg
status: open
deps: [wra-35eb]
links: [wra-8fjd, wra-ylfx, wra-d6vo]
created: 2026-05-16T16:13:56Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-bjaa
tags: [async, tokio, payload, tui, stability]
---
# Define cancellable payload session runner before Tokio payload migration

Problem:
Payload client plain mode, TUI mode, stdin forwarding, signal forwarding, resize handling, and diagnostic sessions use separate blocking loops/threads without a shared cancellation primitive.

Grounding:
- vm-frontend/src/payload_client.rs:140-142 disables payload socket timeouts.
- vm-frontend/src/payload_client.rs:251-303 runs payload session receive loop with stdin/signal helpers.
- vm-frontend/src/payload_client.rs:590 and nearby tests cover protocol behavior.
- vm-frontend/src/tui.rs:677 and surrounding code run TUI payload viewport behavior.
- vm-frontend/src/main.rs:178-201 wires launch payload result into process exit.

Why async helps:
Tokio would allow a single runner to select over payload frames, stdin/input events, resize/signal events, cancellation, and shutdown. It can remove ad hoc helper threads and make stream shutdown explicit.

Can be solved without Tokio:
Partly. Nonblocking/mio or finite read timeouts plus an owned cancellation token, stream shutdown, and joined helper threads are enough for the immediate wra-d6vo hang class.

Proposed implementation shape:
Introduce a PayloadSessionRunner abstraction with existing framing preserved, explicit CancelToken, and structured outcomes: Exit, Failure, Io, Cancelled. Migrate plain payload mode first, then TUI. Decide after tests whether the backend should use Tokio or existing std/mio.

Relationships:
- Prerequisite/parent slice for wra-d6vo.
- Feeds structured outcomes into wra-8fjd.
- Enables wra-ylfx to cancel payload IO before VM shutdown.

Validation:
- Fake server accepts request and never sends exit; assert plain and TUI cancellation terminate and restore terminal state.
- Frame roundtrip tests for partial async reads/writes.
- Run required validation before closing implementation.

