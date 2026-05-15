---
id: wra-crwz
status: closed
deps: [wra-o9y4]
links: []
created: 2026-05-15T10:38:03Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-neci
---
# Harden host ingress and TCP proxy readiness edge coverage

Recent host-ingress bugs showed that readiness APIs need adversarial tests. TCP proxy appears to retain similar risk around one-read-per-event behavior, partial writes, cleanup, and session registration. Host ingress also needs more close/backpressure interleavings beyond the new drain and opportunistic write tests.

## Design

Add table/property tests for host_ingress and tcp_proxy: multiple chunks under one readable event, pending writes across WouldBlock, guest close while host/upstream write is pending, host/upstream EOF after partial write, upstream read errors, guest FIN/RST, large transfers, local port wrap/collision, many concurrent sessions, and session_handles/session_interest cleanup. Where possible connect these tests to the runtime readiness harness.

## Acceptance Criteria

Host ingress and TCP proxy both drain readable sockets to WouldBlock or have explicit tested semantics. Tests prove no lingering sessions/registrations after EOF/error/close. Backpressure and close interleavings preserve or close data intentionally without hangs.


## Notes

**2026-05-15T11:21:58Z**

Started. Hardened TCP proxy readiness semantics so one readable event drains upstream reads until WouldBlock/EOF rather than only one 64KiB read. Added upstream_readiness_drains_response_until_would_block regression test; tcp_proxy targeted suite and full offline validation passed. More close/backpressure interleavings remain.

**2026-05-15T11:24:13Z**

Added TCP proxy stale-session cleanup: sessions are retained only while the gateway still reports an established active TCP session. Added guest_close_removes_proxy_session_and_interest to assert guest close removes proxy session handles/interests. Targeted tcp_proxy suite and full offline validation passed.

**2026-05-15T11:30:00Z**

After appliance rebuild, ./vm-frontend/validate.sh required passed end-to-end. Added final TCP proxy EOF/read-error cleanup tests in this iteration; targeted tcp_proxy suite passed with 12 tests and required passed. Coverage now includes host-ingress drain semantics, TCP proxy drain-to-WouldBlock, guest close cleanup, upstream EOF/read error cleanup, and backpressure retention/flush.
