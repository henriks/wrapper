---
id: wra-d6vo
status: closed
deps: []
links: [wra-8fjd, wra-m7gg, wra-ylfx, wra-35eb, wra-2qij]
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



## Notes

**2026-05-16T16:31:17Z**

Cross-ticket insight from wra-35eb: plain-mode SignalForwarder now has policy-driven local-abort behavior that sets done and shuts down the payload stream for repeated SIGINT/SIGTERM. Remaining d6vo-specific work still includes making the self-pipe nonblocking/CLOEXEC and adding real signal-storm and wedged-server process tests.

**2026-05-16T16:33:05Z**

The signal-safe pipe portion is now implemented under wra-35eb: create_signal_pipe sets O_NONBLOCK and FD_CLOEXEC on both ends; signal handler ignores full-pipe writes; loop handles WouldBlock/Interrupted. Remaining d6vo work should focus on end-to-end SIGTERM/repeated-SIGINT wedged payload server tests and ensuring local abort propagates to frontend cleanup outcomes.

**2026-05-16T16:34:30Z**

End-to-end wedged-server coverage has been added under payload_client tests: fake server accepts payload request and stalls; injected SIGTERM through installed handler terminates run_payload_tcp_with_control. This covers the main d6vo hang class for SIGTERM. Repeated SIGINT forwarding/abort is covered at signal_forward_loop level.

**2026-05-16T21:46:22Z**

Review after wra-35eb/wra-m7gg: the original signal hang/deadlock scope is now implemented. PayloadControlPolicy and SignalForwarder forward first SIGINT/SIGHUP/SIGWINCH, treat repeated SIGINT and SIGTERM as local abort, set the shared done flag, and shut down the payload stream so blocked recv exits. create_signal_pipe sets O_NONBLOCK and FD_CLOEXEC; payload_signal_handler ignores full nonblocking pipes. PayloadSessionRunner/PayloadCancelToken provide the cancellable plain-session path. Regression coverage now includes nonblocking pipe flags, full-pipe signal handler safety, repeated SIGINT forward-then-abort, wedged-server SIGTERM cancellation, and explicit cancel-token interruption. Required validation was recorded as passing under wra-m7gg/related notes.
