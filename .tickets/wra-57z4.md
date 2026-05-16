---
id: wra-57z4
status: open
deps: [wra-bbgh]
links: [wra-t2uv, wra-olu4, wra-i3t9, wra-bbgh, wra-e9sr, wra-nui7]
created: 2026-05-16T15:50:47Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [vmnet, performance, stability]
---
# Bound vmnet per-session buffers and per-tick read drains

Problem:
TCP proxy and host-ingress buffering uses unbounded Vec queues and read loops that drain until WouldBlock. HTTP request parsing also accumulates incomplete request bytes without a hard cap.

Relevant code:
- vm-frontend/src/tcp_proxy.rs:468-474 stores http_buffer and pending upstream/guest buffers.
- vm-frontend/src/tcp_proxy.rs:849-850 appends to http_buffer.
- vm-frontend/src/tcp_proxy.rs:872-890 appends and drains pending upstream bytes.
- vm-frontend/src/tcp_proxy.rs:901-940 drains reads into an unbounded collected Vec.
- vm-frontend/src/host_ingress.rs:207-232 and 270-276 append pending guest/host writes.
- vm-frontend/src/host_ingress.rs:529-546 drains pending host writes.

Impact:
A fast peer, slow peer, or incomplete HTTP request can consume large memory and monopolize a runtime tick, starving other sessions.

Recommended fix:
Define per-session buffer limits and per-tick byte/frame budgets. Stop reading when queued bytes exceed a high watermark, resume when backpressure clears, and fail closed with logged diagnostics if hard limits are exceeded.

Validation:
- Add slow-guest/slow-upstream tests with multi-MB payloads asserting bounded memory and progress for a second session.
- Add deterministic failure tests when limits are exceeded.
- Add fuzz/stress coverage for arbitrary network input/output paths affected by limit handling.

