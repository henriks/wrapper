---
id: wra-57z4
status: open
deps: [wra-bbgh]
links: [wra-t2uv, wra-olu4, wra-i3t9, wra-bbgh, wra-e9sr, wra-nui7, wra-jenv]
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


## Notes

**2026-05-16T19:08:35Z**

wra-nui7 added initial host-ingress per-session buffer limits: HostIngressBufferLimits caps pending_host_write and pending_guest_write with fail-closed HostIngressEvent::BufferLimitExceeded and guest close frames. This does not close wra-57z4 because broader per-session buffer/per-tick drain policy for vmnet/proxy/QEMU remains open, but it provides a concrete host-ingress precedent.

**2026-05-16T19:11:44Z**

wra-nui7 added host listener accept fairness cap: serve_vmnet_gateway now uses HostIngressListenerSet::accept_ready_limited with DEFAULT_HOST_INGRESS_ACCEPTS_PER_LISTENER_PUMP=64 and logs host_ingress_accept_limit_reached. This is a host-ingress accept-drain cap only; broader per-tick read drains and QEMU write backpressure remain on wra-57z4/wra-bbgh.

**2026-05-16T19:13:33Z**

wra-nui7 added a tcp_proxy upstream read cap per owner pass: read_available now takes a max_bytes limit derived from the session pending_guest_bytes watermark. This reduces unbounded per-read memory/drain behavior for established upstream sessions, but wra-57z4 still owns broader per-tick drain policy across vmnet components.

**2026-05-16T19:22:20Z**

Host ingress now has an owner-side per-session host-read byte cap (`HostIngressPumpLimits`) and `host_ingress_host_read_limit_reached` logging. This covers one `wra-nui7` fairness seam; broader per-session/per-tick policy for all vmnet queues remains on this ticket.

**2026-05-16T19:26:20Z**

Host ingress per-owner-pass fairness now covers both directions: host->guest reads and guest->host writes are capped per session via HostIngressPumpLimits, with read/write limit events for event-log visibility. Broader all-vmnet per-tick policy still remains on this ticket.

**2026-05-16T21:47:02Z**

Review after wra-kiv5/wra-nui7 refactors: keep open. DNS and TCP connect blocking are now handled by bounded workers, and host-ingress plus upstream read paths have several owner-pass caps and queue limits, but this ticket still covers the broader all-vmnet per-session/per-tick policy and remains correctly blocked by QEMU write-backpressure work in wra-bbgh. Production byte-IO worker wiring should still wait for wra-bbgh/e9sr rather than treating these partial bounds as complete.
