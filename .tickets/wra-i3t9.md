---
id: wra-i3t9
status: closed
deps: []
links: [wra-t2uv, wra-57z4, wra-olu4, wra-bbgh, wra-kiv5]
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


## Notes

**2026-05-16T17:11:12Z**

wra-kiv5 added a typed TcpConnect service command/completion envelope keyed by VmnetServiceToken. It intentionally does not move TcpProxyBridge yet; next seam should split connect decision/session pending state from blocking connector.connect so failures can be applied owner-side as guest-visible close/reset.

**2026-05-16T17:15:31Z**

Partial fix landed under wra-kiv5: synchronous upstream connect failures now call gateway.close_tcp_session, include guest_frames on TcpProxyEvent::ConnectFailed, and vmnet_runtime writes those frames. Remaining i3t9 work is to make the connect operation itself nonblocking/worker-backed and ensure async failure completions use the same owner-side close path.

**2026-05-16T17:18:23Z**

wra-kiv5 now has TcpProxyBridge::plan_connect / complete_connect. plan_connect can produce a pending connect without invoking connector.connect, and complete_connect owns success insertion or guest-visible close on failure. This is the seam to move connector.connect to a bounded worker in the remaining nonblocking-connect work.

**2026-05-16T17:58:46Z**

wra-kiv5 now has a TCP connect worker-side executor primitive. It returns typed TcpConnect completions with connector success/failure only; owner-side completion application still needs to reuse TcpProxyBridge::complete_connect so guest close/reset semantics remain on the vmnet owner.

**2026-05-16T18:04:22Z**

TCP connect completion application now explicitly reuses TcpProxyBridge::complete_connect on the owner. This preserves existing guest close/reset behavior on async connect failure; runtime worker wiring still needs to submit pending connects and drain completions.

**2026-05-16T21:46:35Z**

Review after wra-kiv5 async-service refactor: this bug's concrete TCP connect scope is now complete. TcpProxyBridge now splits plan_connect/complete_connect; serve_vmnet_gateway submits pending connects through a bounded spawned TCP connect worker and drains completions on ServiceIo wakeups; failed async completions reuse complete_connect and close/reset the guest session with guest frames. Regressions cover connect failure guest-visible closure, panic-on-sync connector proving pending connects skip synchronous connect, and a slow blocked connect worker that does not prevent an unrelated TCP SYN from progressing. wra-kiv5 recorded required validation passing, including live tiers in the required gate.
