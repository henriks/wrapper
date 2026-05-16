---
id: wra-t2uv
status: open
deps: []
links: [wra-57z4, wra-olu4, wra-i3t9, wra-bbgh]
created: 2026-05-16T16:13:56Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-bjaa
tags: [async, tokio, vmnet, architecture]
---
# Define vmnet async service-IO boundary around single smoltcp owner

Problem / decision to make:
Several vmnet issues would benefit from async service IO, but a wholesale Tokio rewrite could split smoltcp ownership or reorder guest-visible frames. Define the async boundary before implementing Tokio-backed DNS/connect/session work.

Grounding:
- vm-frontend/src/vmnet_runtime.rs:179-331 has one synchronous runtime loop owning QEMU, host ingress, proxy pumping, event logging, and poll registration.
- vm-frontend/src/guest_tcp.rs:19-24, 86-93, and 267-355 show GuestTcpCore, SocketSet, and QueuedEthernetDevice as a natural single-owner smoltcp core.
- vm-frontend/Cargo.toml currently uses mio and has no Tokio dependency.

Proposed architecture:
Keep one VmnetOwner loop/task that owns VmnetGateway, GuestTcpCore, QEMU frame ordering, pcap capture, and guest-visible close/reset semantics. Use async tasks only around service IO: DNS upstream, outbound TCP connect, host listener/session sockets, and possibly upstream TCP read/write. Communicate through bounded command/completion channels with explicit full-channel semantics.

What remains synchronous:
smoltcp Interface/SocketSet, TCP state transitions, policy decisions tied to guest frames, QEMU frame serialization order, pcap capture ordering, and event-log formatting.

Risks:
A naive Tokio rewrite can introduce frame reordering, hidden unbounded queues, shutdown leaks, or missed wakeups between async completions and the existing poll loop.

Validation:
- Architecture-level unit tests for owner command ordering and full-channel behavior.
- Existing required gate remains ./vm-frontend/validate.sh required.
- Live validation is required before closing implementation tickets that change runtime behavior.

