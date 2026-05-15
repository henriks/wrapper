---
id: wra-o9y4
status: closed
deps: [wra-gx6d]
links: []
created: 2026-05-15T10:37:51Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-neci
---
# Add deterministic mio runtime readiness integration harness

The real serve_vmnet_gateway loop dynamically registers QEMU stream, host listeners, host sessions, and upstream sessions with RuntimePoller. Existing tests mostly cover helper pumps and basic poller operations, which missed event ordering bugs such as data arriving before session registration.

## Design

Introduce an injectable or testable runtime loop abstraction around RuntimePoller, QemuFrameIo, host listener readiness, host session readiness, upstream readiness, and smoltcp timeouts. Use real nonblocking socketpairs/fds or a fake poller to script QEMU readable, host listener readable, host session readable/writable, upstream readable/writable, empty poll timeout, close/error readiness, and mixed events in one tick. Assert registration lifecycle and interest changes after buffers fill/drain.

## Acceptance Criteria

Offline tests exercise serve_vmnet_gateway-equivalent readiness dispatch, not only pump helpers. Coverage includes pre-registration data, stale readiness, reregister interest changes, deregister on close/error, timer-only TCP progress, and simultaneous QEMU/host/proxy readiness.


## Notes

**2026-05-15T11:02:12Z**

Started and implemented first runtime readiness dispatch seam. Extracted RuntimeReadyDispatch from serve_vmnet_gateway event classification so QEMU readable/closed, host listener readable, host session read/write/error/close, upstream read/write/error/close, simultaneous events, and empty timer-only polls are deterministic unit-testable. Added runtime_ready_dispatch tests and verified vmnet_runtime::tests plus validate fast/fuzz-check. This is a foundation but ticket remains open for registration lifecycle/integration harness coverage.

**2026-05-15T11:06:47Z**

Added RuntimePoller registration lifecycle coverage. New tests cover data arriving before registration, same-source fd replacement ignoring stale readiness from old fd, reregistered interest changing ready events, and assertions for registration interest/fd/count. Targeted validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_poller::tests:: -- --nocapture passed. Combined with RuntimeReadyDispatch tests, offline coverage now includes pre-registration data, stale readiness, interest changes, close/error classification, timer-only empty polls, and simultaneous QEMU/host/proxy readiness. Ticket remains open pending live/required validation blocked by stale appliance artifacts.

**2026-05-15T11:08:47Z**

Validation after poller lifecycle additions: docs/fmt/fast/fuzz-check passed; host-live remains blocked by stale appliance manifest. Current offline readiness coverage includes RuntimeReadyDispatch and RuntimePoller lifecycle tests; ticket remains open for full required/live evidence after rebuild.

**2026-05-15T11:30:00Z**

After appliance rebuild, ./vm-frontend/validate.sh required passed end-to-end. Offline coverage includes RuntimeReadyDispatch simultaneous/close/error/timer-only classification plus RuntimePoller pre-registration, stale fd replacement, reregistered interest, fd/count assertions. Live self-test ok exercises runtime path with rebuilt appliance.
