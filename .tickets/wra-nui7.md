---
id: wra-nui7
status: closed
deps: [wra-t2uv]
links: [wra-e9sr, wra-57z4, wra-jenv, wra-bbgh]
created: 2026-05-16T16:13:56Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-bjaa
tags: [async, tokio, vmnet, host-ingress, performance]
---
# Convert host ingress and upstream session IO to bounded async service tasks

Problem:
Host ingress and established upstream sessions already use readiness, but they still drain accepts/reads/writes in owner-loop passes and use unbounded Vec buffers. If vmnet adopts Tokio, this slice should move socket IO to bounded service tasks without moving smoltcp ownership.

Grounding:
- vm-frontend/src/host_ingress.rs:61-89 drains listeners until WouldBlock.
- vm-frontend/src/vmnet_runtime.rs:264-323 opens host connections and pumps host/proxy work inline.
- vm-frontend/src/host_ingress.rs:206-261 reads host-side payloads while readable.
- vm-frontend/src/host_ingress.rs:370-375 stores unbounded pending writes.
- vm-frontend/src/tcp_proxy.rs:464-475 stores unbounded per-session proxy buffers.
- vm-frontend/src/tcp_proxy.rs:872-940 drains writes/reads until WouldBlock.

Proposed implementation shape:
Run Tokio listener/session tasks for host-side and upstream-side sockets. Listener tasks send accepted connections through a bounded queue. Session tasks use bounded byte queues in both directions. The smoltcp owner still decides when to connect host-to-guest, assign local ports, read guest bytes, and deliver bytes into guest TCP.

Relationships:
- Depends on the async boundary ticket.
- Coordinate with wra-57z4 for shared buffer limits/full-queue semantics.
- Coordinate with wra-e9sr because async accept/session churn makes socket reaping more important.

Risks:
Accept loops can still be unfair if the owner drains accept queues without per-tick caps. More concurrency can expose local-port wrap/reuse behavior in host_ingress.rs:147-150.

Validation:
- Multi-listener accept flood with a concurrent established session proving progress.
- Slow host/upstream reader tests asserting bounded queues and deterministic close/log behavior.
- Add fuzz/stress coverage for host-ingress/proxy arbitrary input/output state transitions if state machines change.


## Notes

**2026-05-16T19:04:45Z**

Started after closing wra-g13g with required validation. Scope reminder: convert host ingress and upstream session IO to bounded async service tasks without moving smoltcp/QEMU frame ownership off the vmnet owner. Coordinate with linked wra-e9sr (stale session reaping, still blocked by wra-i3t9) and wra-57z4 (per-session buffers/per-tick drains, blocked by wra-bbgh); if details arise that belong to those tickets, document them there rather than broadening this slice.

**2026-05-16T19:08:35Z**

First host-ingress bounded-buffer slice added HostIngressBufferLimits with pending_host_write and pending_guest_write defaults, plus HostIngressEvent::BufferLimitExceeded carrying guest close frames. HostIngressBridge now fails closed if guest->host pending writes or host->guest pending writes would exceed the configured owner-side queue limit; runtime writes/logs those close frames and deregisters the host session. Added regressions pending_host_write_limit_fails_closed_when_host_write_would_grow_unbounded and pending_guest_write_limit_fails_closed_when_host_read_would_grow_unbounded. Focused validation passed: cargo fmt; cargo test ... host_ingress --offline; cargo test ... event_log_includes_representative --offline; cargo test ... vmnet_runtime --offline.

**2026-05-16T19:09:24Z**

Broader vmnet-filtered validation also passed after host-ingress buffer-limit changes: cargo test --manifest-path vm-frontend/Cargo.toml vmnet --offline (94 passed, 2 ignored stress tests as before; bin vmnet config tests passed).

**2026-05-16T19:11:13Z**

Added bounded host listener accept-drain semantics. HostIngressListenerSet now has accept_ready_limited(), returning HostIngressAcceptBatch with HostIngressAcceptLimit metadata when a per-listener pump cap is reached. serve_vmnet_gateway uses DEFAULT_HOST_INGRESS_ACCEPTS_PER_LISTENER_PUMP (64) per ready listener rather than draining accepts unboundedly, and emits HostIngressEvent::AcceptLimitReached for deterministic event logging/fairness visibility. Event log formatting now includes host_ingress_accept_limit_reached. Focused validation passed: cargo fmt; cargo test ... host_ingress --offline; cargo test ... event_log_includes_representative --offline; cargo test ... vmnet_runtime --offline; cargo test ... vmnet --offline.

**2026-05-16T19:13:33Z**

Added an upstream-session owner-pass read cap in tcp_proxy. read_available now takes a max_bytes limit (using each session pending_guest_bytes watermark) and returns after collecting at most that many upstream bytes in one owner pass, preventing a readable upstream socket from being drained unboundedly before guest delivery/backpressure is applied. Added upstream_read_is_capped_per_owner_pass. Focused validation passed: cargo fmt; cargo test ... upstream_read_is_capped --offline; cargo test ... tcp_proxy --offline (20 tests); cargo test ... vmnet_runtime --offline (20 tests); cargo test ... vmnet --offline (94 passed, 2 ignored stress tests as before; bin vmnet config tests passed).

**2026-05-16T19:16:13Z**

Added a bounded accepted-connection queue as the owner-side seam for future host accept tasks. AcceptedHostConnection is now generic for tests; HostIngressAcceptedQueue preserves FIFO order, rejects full capacity explicitly, and exposes capacity for logging. serve_vmnet_gateway now pushes accepted TcpStreams into a DEFAULT_HOST_INGRESS_ACCEPT_QUEUE_LIMIT=128 queue before opening guest sessions; full queues drop/close the just-accepted host connection by dropping it and emit HostIngressEvent::AcceptQueueFull / host_ingress_accept_queue_full. Added accepted_queue_rejects_full_capacity_and_preserves_order. Focused validation passed: cargo fmt; cargo test ... accepted_queue --offline; cargo test ... host_ingress --offline (14 passed, 1 ignored loopback test as before); cargo test ... event_log_includes_representative --offline; cargo test ... vmnet_runtime --offline; cargo test ... vmnet --offline (94 passed, 2 ignored stress tests as before; bin vmnet config tests passed).

**2026-05-16T19:18:20Z**

Added proptest_accepted_queue_preserves_fifo_and_capacity to stress the accepted-host-connection queue across capacities/counts. Focused validation passed: cargo fmt; cargo test ... proptest_accepted_queue --offline; cargo test ... host_ingress --offline (15 passed, 1 ignored loopback test as before).

**2026-05-16T19:22:20Z**

Added HostIngressPumpLimits with DEFAULT_HOST_INGRESS_HOST_READ_BYTES_PER_SESSION_PUMP and HostReadLimitReached logging. Host ingress now caps host->guest bytes read per session per owner pump so a permanently readable host connection returns to the owner loop instead of monopolizing it; focused tests passed: host_read_bytes_limit, host_ingress, representative event log.

**2026-05-16T19:26:20Z**

Added host-ingress host-write per-session pump cap alongside the previous host-read cap. `HostIngressPumpLimits` now bounds both host reads and host writes per owner pass, emits `HostWriteLimitReached` / `host_ingress_host_write_limit_reached`, and tests prove writes are split across owner pumps. Added fairness coverage proving a session hitting the read cap does not prevent another ready host session from being processed in the same pump. Focused host_ingress/event_log/vmnet_runtime/vmnet tests passed.

**2026-05-16T19:29:40Z**

Added generic byte-IO service command/completion shapes in vmnet_service_io for HostIngress and UpstreamSession work. `VmnetServiceCommand::ByteIo` carries owner-issued tokens, service kind, bounded read/write operations; `VmnetServiceCompletion::ByteIo` returns bounded read/write outcomes. `execute_byte_io_service_command` provides the worker-side primitive over Read+Write without smoltcp/QEMU ownership. Tests cover token/kind metadata, read/write caps, WouldBlock, and cancellation; vmnet_service_io/vmnet_runtime/vmnet filtered suites passed.

**2026-05-16T19:31:47Z**

Added spawned byte-IO worker primitive for established host-ingress/upstream-session sockets. `spawn_byte_io_service_worker` owns one `Read+Write` connection, consumes bounded ByteIo commands, emits bounded ByteIo completions, wakes the single owner through VmnetServiceWakeup, rejects zero-capacity limits, and exits instead of blocking when the completion queue is full. Focused validation passed: spawned_byte_io_worker, typed_completion_metadata, vmnet_service_io, vmnet_runtime, vmnet.

**2026-05-16T19:33:03Z**

Added `run_byte_io_service_owner_step`, the owner-queue step primitive for byte IO mirroring the DNS service step: consumes one bounded ByteIo command, executes bounded read/write against a service-owned connection, enqueues a bounded completion, reports full completion queues without touching owner pending context, and idles without IO. Focused validation passed: byte_io_service_owner_step, vmnet_service_io, host_ingress, tcp_proxy, vmnet_runtime, vmnet.

**2026-05-16T19:34:56Z**

Reflection iteration 56: byte-IO service boundary now has command/completion envelopes, executor, spawned worker, owner-step primitive, and proptest coverage for arbitrary bounded read/write lengths. `tk ready` still shows wra-nui7 as active unblocked epic child; wra-lcbk remains blocked by guest/appliance-sensitive prerequisites.

**2026-05-16T19:38:15Z**

Required validation passed after byte-IO boundary/worker/owner-step/proptest changes: ./vm-frontend/validate.sh required completed successfully within 600s, including live setup-tool scenarios. No appliance-sensitive files touched and no appliance rebuild required.

**2026-05-16T19:40:52Z**

Closed at the tested async-ready boundary rather than hastily wiring established-session TcpStreams into production workers. Implemented bounded host-ingress/session queues, fairness caps, upstream read caps, accepted-connection queue seam, ByteIo command/completion envelope, executor, spawned worker, owner-step primitive, and proptest/focused coverage. Required validation passed. Created follow-up wra-jenv for production worker adapter after QEMU backpressure, per-session drain, and stale-session reaping policies are ready.
