---
id: wra-i3t9
status: open
deps: []
links: []
created: 2026-05-16T15:50:47Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-piqm
tags: [vmnet, tcp, stability]
---
# Make upstream TCP connect nonblocking and close failed guest sessions

Problem:
Outbound upstream TCP connect uses blocking connect_timeout in the vmnet loop. Connect failures are reported as events but the guest TCP session is not always closed/reset in a guest-visible way, allowing retries or hangs.

Relevant code:
- vm-frontend/src/tcp_gateway.rs:80-85 calls TcpStream::connect_timeout.
- vm-frontend/src/vmnet_runtime.rs:186-192 wires the connector into the runtime.
- vm-frontend/src/tcp_proxy.rs:105-167 creates proxy sessions and handles connect failures.

Impact:
A single slow connect can block the vmnet runtime for the timeout, and failed connects can leave the guest with an established TCP session that never behaves like a failed connection.

Recommended fix:
Make upstream connect nonblocking/readiness-driven or move it to a bounded connector worker. On connect failure, close or reset the guest TCP session and retire the proxy attempt.

Validation:
- Extend existing connect-failure tests to assert guest closure frames/session cleanup.
- Add a slow connector test proving other sessions progress while one connect is pending.
- Exercise with live smoke if connection behavior changes guest-visible networking.

