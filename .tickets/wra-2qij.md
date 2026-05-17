---
id: wra-2qij
status: closed
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


## Notes

**2026-05-16T16:40:54Z**

Cross-ticket insight from wra-m7gg: PayloadSessionRunner now has a real PayloadCancelToken that shuts down the registered payload stream and returns PayloadSessionOutcome::Cancelled. Diagnostic/shutdown deadline work can mirror this shape for run_diagnostic_tcp_with_deadline rather than relying on disabled socket timeouts.

**2026-05-16T16:55:53Z**

Started after wra-m7gg closure. Available foundation: PayloadCancelToken can interrupt blocked payload TcpStream reads, PayloadSessionRunner returns structured Exit/Failure/Cancelled, and cancellation/partial-frame tests exist. Next slice should apply the same deadline/cancellation shape to diagnostics via run_diagnostic_tcp_with_deadline without changing guest diagnostic frame format.

**2026-05-16T16:57:48Z**

Added run_diagnostic_tcp_with_deadline in vm-frontend/src/payload_client.rs. run_diagnostic_tcp now uses a client-side deadline derived from DiagnosticRequest.timeout_seconds instead of disabling socket read/write timeouts. Timeout-like IO errors are mapped to PayloadClientError::DeadlineExceeded. Added fake partial-frame stall regression run_diagnostic_tcp_with_deadline_fails_partial_frame_stall. Focused validation passed: cargo fmt; cargo test ... run_diagnostic_tcp_with_deadline --offline; cargo test ... run_diagnostic_sends --offline; cargo test ... payload_client --offline.

**2026-05-16T16:59:13Z**

Wired bounded sync into self-test shutdown: run_self_test now calls flush_guest_filesystems(payload_addr) before terminate() and treats sync/deadline failures as fatal with artifact context. This uses the new run_diagnostic_tcp timeout behavior. Focused validation passed: cargo fmt; cargo test ... flush_guest_filesystems --offline; cargo test ... guest_sync_diagnostic_request --offline; cargo test ... self_test --offline.

**2026-05-16T17:01:47Z**

Required validation passed after diagnostic deadline and self-test bounded-sync changes: ./vm-frontend/validate.sh required completed within 600s timeout, including live setup-tool scenarios. No appliance or guest asset changes were made. True graceful guest/QMP poweroff remains tracked in linked wra-ylfx.
