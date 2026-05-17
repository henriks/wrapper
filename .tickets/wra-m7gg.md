---
id: wra-m7gg
status: closed
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


## Notes

**2026-05-16T16:31:17Z**

Cross-ticket insight from wra-35eb: PayloadControlPolicy/Action now provides the signal/resize/escalation decision layer for a future PayloadSessionRunner. Runner should use the same actions but replace ad hoc threads with an explicit cancellation primitive and structured Cancelled outcome.

**2026-05-16T16:37:35Z**

Started after wra-35eb closure. Existing groundwork: PayloadControlPolicy/Action handles signal/resize/escalation; SignalForwarder can locally abort blocked recv by setting done and shutting down stream. Next slice should introduce an explicit PayloadSessionRunner/CancelToken and structured outcomes without changing guest frame protocol.

**2026-05-16T16:39:50Z**

Introduced host-side PayloadCancelToken, PayloadSessionRunner, and PayloadSessionOutcome in vm-frontend/src/payload_client.rs. Plain run_payload_session now delegates through the runner while preserving the guest frame protocol and public i32 exit API. Runner returns structured Exit/Failure/Cancelled outcomes; public wrapper maps Cancelled to PayloadClientError::Cancelled. Added focused tests for structured Failure outcome, partial-frame IO error, and existing SIGTERM cancellation path now returning Cancelled. Verified with cargo fmt and focused offline tests: payload_session_runner and signal_forwarder_terminates_blocked_payload_receive_on_sigterm.

**2026-05-16T16:40:54Z**

Improved PayloadCancelToken so external cancellation is real, not just an atomic flag: runner registers a cloned TcpStream, and PayloadCancelToken::cancel sets the flag and shutdowns the stream to interrupt a blocked recv. Added payload_session_runner_cancel_token_interrupts_blocked_receive fake wedged-server test. Focused validation passed: cargo fmt and cargo test --manifest-path vm-frontend/Cargo.toml payload_session_runner --offline.

**2026-05-16T16:52:52Z**

Fixed a race in signal_forwarder_terminates_blocked_payload_receive_on_sigterm: the test could inject SIGTERM after the fake server saw the request but before SignalForwarder stored SIGNAL_WRITE_FD, dropping the signal and hanging. Added wait_for_signal_forwarder_installed before injection. Focused test and full vm-frontend offline tests now pass with short timeouts. Avoiding long validation timeouts until focused suites are stable.

**2026-05-16T16:55:53Z**

Required validation passed after runner/cancel-token changes: ./vm-frontend/validate.sh required completed within 600s timeout, including live setup-tool scenarios. Scope decision: TUI continues to use its existing payload-reader thread for now; the new PayloadCancelToken/Runner is a plain-mode foundation and exposes the cancellation/outcome semantics needed before any Tokio/TUI migration. No guest protocol or appliance changes were made.
