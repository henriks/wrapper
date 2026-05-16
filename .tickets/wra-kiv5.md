---
id: wra-kiv5
status: open
deps: [wra-t2uv]
links: [wra-olu4, wra-i3t9, wra-bbgh]
created: 2026-05-16T16:13:56Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-bjaa
tags: [async, tokio, vmnet, dns, tcp]
---
# Move vmnet DNS and outbound connect onto bounded async IO tasks

Problem:
DNS upstream exchange and outbound TCP connect are the clearest blocking calls inside the vmnet path. Tokio can help, but only if completion and backpressure semantics are explicit and bounded.

Grounding:
- vm-frontend/src/dns_proxy.rs:74 and 105-123 perform DNS upstream exchange inline with blocking UdpSocket behavior.
- vm-frontend/src/vmnet_gateway.rs:213 and 463 show DNS handling and the default 5 second upstream timeout.
- vm-frontend/src/tcp_gateway.rs:30 defines a synchronous TcpUpstreamConnector.
- vm-frontend/src/tcp_gateway.rs:80 calls blocking TcpStream::connect_timeout.
- vm-frontend/src/tcp_proxy.rs:105-167 starts connects inline while processing guest sessions.

Proposed implementation shape:
Split synchronous policy/parse from async IO. For DNS, parse and classify synchronously, enqueue allowed upstream lookups, and synthesize guest frames on async completion with bounded timeout/SERVFAIL behavior. For TCP, introduce PendingConnect state keyed by smoltcp handle, start async connect with timeout/cancellation, and only create an upstream session on success. On failure, close/reset the guest TCP session and enqueue resulting frames.

Dependencies / relationships:
- Depends on the async boundary ticket under wra-bjaa.
- Link to wra-olu4 for DNS blocking and wra-i3t9 for blocking connect/guest-visible failure.
- Depends on or coordinates with wra-bbgh so async completions have a safe QEMU frame emission path.

Risks:
Out-of-order DNS responses, duplicate DNS IDs, duplicate connect attempts, cancellation during shutdown, and ownership conversion between Tokio sockets and existing std/mio session code.

Validation:
- Fake delayed/blackholed DNS upstream with unrelated TCP/host-ingress progress assertions.
- Slow connect regression proving a second guest session progresses while one connect is pending.
- Connect failure asserts guest-visible close/reset frames and session cleanup.
- Run live smoke because DNS/TCP timing is guest-visible.

