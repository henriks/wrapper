---
id: wra-662v
status: open
deps: [wra-cvmy, wra-n0fe]
links: [wra-y335]
created: 2026-05-17T10:19:50Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, guest-service, payload]
---
# Option 1: implement opt-in Rust Tokio guest payload service

Build guest-service into the real Rust payload server using the shared payload protocol and make it usable through the existing opt-in appliance path. guest-service/src/main.rs is currently a skeleton. Do not remove or replace the Python guest payload server in the default runtime path in this ticket; wra-y335 owns parity/live validation before any default switch.

## Design

Implement a Tokio TCP server that owns child process lifecycle, stdout/stderr streaming, stdin forwarding, resize/signal handling, diagnostic timeouts, output limits, cancellation, and failure reporting. Reuse the shared protocol crate and preserve the bounded Python baseline semantics documented by wra-y335/wra-lcbk: PTY/session/process-group behavior, quiet long-running primary payloads, signal/resize/stdin frames, diagnostic limits/timeouts, client/session caps, and slow-writer cleanup. Leave Python as the default/fallback until the explicit validation/default-switch path completes.

## Acceptance Criteria

The appliance can use the Rust guest service for payload execution in opt-in mode, protocol tests are shared with frontend, focused payload tests pass for the Rust service, and wra-y335 remains the blocking parity/live validation ticket before any default switch or Python removal.


## Notes

**2026-05-17T11:03:02Z**

Relationship to existing hardening ticket wra-y335: this ticket should implement the real Rust/Tokio guest payload service and make it usable in opt-in appliance mode. wra-y335 is the parity/live validation gate before any default switch. Scope correction: do not remove the Python guest service from the production/default path in this ticket; that should happen only after wra-y335 passes and a separate default-switch/removal ticket is created or selected.
