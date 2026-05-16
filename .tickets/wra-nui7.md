---
id: wra-nui7
status: open
deps: [wra-t2uv]
links: [wra-e9sr, wra-57z4]
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

