---
id: wra-d6vo
status: open
deps: []
links: []
created: 2026-05-16T15:50:47Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-piqm
tags: [frontend, payload, tui, stability]
---
# Make payload signal forwarding cancellable and signal-safe

Problem:
Plain payload mode installs process-wide handlers for SIGINT, SIGTERM, and SIGHUP, forwards them to the guest, and then blocks indefinitely waiting for payload events. The signal self-pipe is created as blocking, so a signal storm can block inside the signal handler.

Relevant code:
- vm-frontend/src/payload_client.rs:140-142 disables read/write timeouts for payload sessions.
- vm-frontend/src/payload_client.rs:287-299 blocks in recv_event.
- vm-frontend/src/payload_client.rs:417, 472, and 505 cover signal pipe/handler behavior.
- vm-frontend/src/tui.rs:593 has related TUI signal/control behavior.

Impact:
If the guest or payload control path wedges, Ctrl-C/SIGTERM may not terminate the wrapper. A signal storm can deadlock or make shutdown unreliable.

Recommended fix:
Use pipe2(O_NONBLOCK | O_CLOEXEC) and coalesce/ignore EAGAIN in the handler. Add local cancellation semantics: first Ctrl-C forwards, second Ctrl-C or SIGTERM aborts locally and runs frontend cleanup. Consider read interruption instead of indefinite blocking reads.

Validation:
- Fake payload server that accepts request and never sends exit; assert SIGTERM and repeated SIGINT terminate and restore handlers.
- Signal-storm regression for nonblocking self-pipe behavior.


