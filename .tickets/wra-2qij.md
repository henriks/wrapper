---
id: wra-2qij
status: open
deps: [wra-m7gg]
links: [wra-8fjd, wra-ylfx, wra-d6vo]
created: 2026-05-16T16:13:56Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-bjaa
tags: [async, tokio, payload, shutdown, diagnostics]
---
# Coordinate diagnostic deadlines with async-ready graceful shutdown

Problem:
Diagnostic requests carry guest-side timeout_seconds, but the client disables socket timeouts and can block forever if the guest control service wedges mid-frame. Shutdown then proceeds through best-effort sync and hard QEMU kill paths.

Grounding:
- vm-frontend/src/payload_client.rs:201-210 starts diagnostic TCP and disables socket timeouts.
- vm-frontend/src/payload_client.rs:324-340 reads diagnostic frames until exit/failure.
- vm-frontend/src/main.rs:1740, 2039, and 2054 include diagnostic/payload paths used by self-test/launch flows.
- vm-frontend/src/launch.rs:174-183 force terminates QEMU today.

Why async helps:
Tokio can coordinate diagnostic sync, payload cancellation, QEMU wait, and shutdown timeout in one flow.

Can be solved without Tokio:
Yes. Add client-side diagnostic deadlines with socket timeouts or nonblocking polling, then implement graceful shutdown with current wait-timeout/process handling.

Proposed implementation shape:
Create run_diagnostic_tcp_with_deadline and use it for filesystem flush/sync operations. Add shutdown_frontend API: cancel payload session, run sync diagnostic with client deadline, request graceful guest/QEMU shutdown when available, wait bounded time, then force kill. Keep terminate() as explicit force path or rename semantics clearly.

Relationships:
- Complements wra-ylfx, which owns graceful shutdown behavior.
- Uses cancellation primitives from payload runner/signal-policy tickets.
- wra-8fjd should surface structured shutdown/payload failures after terminal restore.

Validation:
- Fake diagnostic server sends partial frame then stalls; assert client deadline fires.
- Launch test where payload exits and sync succeeds before terminate.
- Shutdown test where sync hangs and force-kill still reaps QEMU and writes launch state.
- Live validation should include live-setup-tools and required gate before closing.

