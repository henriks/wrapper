---
id: wra-jenv
status: open
deps: [wra-bbgh, wra-57z4, wra-e9sr]
links: [wra-e9sr, wra-57z4, wra-bbgh, wra-nui7]
created: 2026-05-16T19:40:52Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [async, vmnet, host-ingress, performance]
---
# Wire established-session byte-IO workers into vmnet runtime after owner/backpressure policy is complete

Context from wra-nui7 / wra-bjaa: host ingress and upstream established-session IO now have owner-side bounded queues/fairness caps and an explicit byte-IO service boundary (VmnetServiceCommand::ByteIo, execute_byte_io_service_command, spawn_byte_io_service_worker, run_byte_io_service_owner_step). Those primitives are validated, but production serve_vmnet_gateway wiring was intentionally deferred because moving established TcpStreams into workers affects fd/readiness ownership, deterministic fail-closed cleanup, busy-loop avoidance, stale session reaping, and QEMU stream write backpressure.

Implement the narrower production adapter only after the owner-side backpressure/reaping policies are ready. Preserve the single smoltcp owner: workers may own host/upstream OS sockets and exchange bounded byte commands/completions, but smoltcp state, guest TCP close/reset frames, pcap/logging, and QEMU writes remain owner-side.

## Design

Design requirements:
- Define one owner-driven session-task adapter for host-ingress and/or upstream sessions; avoid duplicate mio registration or simultaneous reads on the same fd.
- Use bounded command/completion queues and VmnetServiceWakeup-style owner notification; full queues must fail closed deterministically and leave no stale pending context.
- Integrate with QEMU write backpressure handling rather than buffering unbounded guest frames.
- Integrate stale/closed session reaping so worker disconnect/cancel cannot leave orphaned sessions.
- Keep Tokio optional until the std-thread boundary proves insufficient; a Tokio implementation must sit behind the same typed boundary.

## Acceptance Criteria

Acceptance criteria:
- Production runtime can delegate established host-ingress/upstream socket byte IO through bounded service workers without moving smoltcp ownership.
- Tests cover slow host/upstream reader, worker disconnect/full completion queue, stale completion/cancel, accept-flood with concurrent established-session progress, and no busy wakeup loop.
- Focused fuzz/proptest/stress coverage exercises arbitrary bounded byte IO/state transitions.
- Required validation passes, plus relevant live vmnet/setup tiers if behavior changes are live-observable.

