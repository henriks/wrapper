---
id: wra-57z4
status: closed
deps: [wra-bbgh]
links: [wra-t2uv, wra-olu4, wra-i3t9, wra-bbgh, wra-e9sr, wra-nui7, wra-jenv, wra-73tn, wra-f762, wra-ynx7, wra-wv0w]
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

**2026-05-18T05:39:48Z**

Cleanup epic wra-9m5h adds wra-ynx7 as a follow-up structural cleanup. While bounding buffers and per-tick drains, avoid adding a second readiness/backpressure model; shape the fix so wra-ynx7 can collapse duplicated HostIngress/TcpProxy accounting instead of preserving it.

**2026-05-18T08:23:40Z**

Cleanup-loop dependency note from wra-bbgh: treat production QEMU writes as backpressured by awaiting write_frame_async rather than adding another guest-bound queue. wra-57z4 should focus on per-session buffers and per-owner-pass read/write budgets in host-ingress/TcpProxy paths, plus tests for slow peer fairness. Do not design a second QEMU readiness/backpressure model only to preserve sync vmnet helper scaffolding.

**2026-05-18T09:00:14Z**

Iteration 34 cleanup-prerequisite progress: added a tcp-proxy HTTP request accumulation cap (`TcpProxyBufferLimits::http_request_bytes`, default 64 KiB). Incomplete intercepted HTTP/HTTPS request bytes are now checked before append; overflow emits `TcpProxyEvent::BufferLimitExceeded` with `HttpRequestBytes`, closes/fails closed via the existing helper, and does not grow the session `http_buffer`. Added regression `tcp_proxy::tests::incomplete_http_header_limit_fails_closed_before_buffering_unbounded`.

Validation for this slice: targeted new test passed; full `tcp_proxy::tests` passed; targeted `vmnet_runtime::tests::event_log_includes_representative_failure_artifacts_without_secrets` passed during the split turn; `./vm-frontend/validate.sh fast` passed in the split turn; first required validation found a pre-existing property-test overreach (`proptest_default_policy_denied_syns_do_not_create_sessions` generated invalid dst_port=0, which smoltcp ignores rather than treating as a policy-denied SYN), so the proptest domain was narrowed to nonzero TCP ports and the targeted test was rerun. Final `./vm-frontend/validate.sh required` passed: /tmp/pi-bash-22b3f93b38d6ca41.log.

Keep ticket open: this closes the unbounded incomplete HTTP request buffer seam, but the ticket still needs a final all-vmnet audit/closure decision for per-session buffers and per-tick read drains before unblocking `wra-jenv`/`wra-ynx7`.

**2026-05-18T09:02:36Z**

Final cleanup-loop audit before closure: the vmnet per-session/per-owner-pass bounds now cover the originally identified seams without adding a parallel QEMU/session worker model. Host ingress has bounded pending host/guest writes, bounded accept queues, per-listener accept caps, and per-session host read/write pump budgets. TcpProxy has bounded pending upstream bytes, pending upstream plaintext, pending guest bytes, a capped upstream read per owner pass, and the new 64 KiB incomplete HTTP request accumulation cap. DNS/TCP connect blocking is already off-owner via bounded Tokio service tasks; QEMU writes remain owner-side with awaited `write_frame_async` backpressure per closed `wra-bbgh`; closed socket/slot reaping is centralized per closed `wra-e9sr`. Current `wra-57z4` code changes passed required validation at `/tmp/pi-bash-22b3f93b38d6ca41.log`.

Closure decision: do not add established-session byte-IO workers here. With the current owner-driven fd readiness plus bounded per-session pumps, the buffer/drain correctness target is met; adding another established-session worker layer would recreate transitional code that cleanup child `wra-ynx7` is supposed to remove.
