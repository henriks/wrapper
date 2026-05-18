---
id: wra-jenv
status: closed
deps: [wra-bbgh, wra-57z4, wra-e9sr]
links: [wra-e9sr, wra-57z4, wra-bbgh, wra-nui7, wra-73tn, wra-f762, wra-ynx7]
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


## Notes

**2026-05-18T05:39:51Z**

Cleanup epic wra-9m5h adds wra-ynx7 after the active vmnet behavior work. Wire byte-IO workers in a way that leaves one owner-driven Tokio boundary; do not introduce transitional sync wrappers or nested runtimes that wra-ynx7 will immediately need to remove.

**2026-05-18T08:23:40Z**

Cleanup-loop dependency note from wra-bbgh: established-session byte-IO worker wiring should continue to leave QEMU writes owner-side and bounded by async write_frame_async backpressure. If workers can produce guest-bound completions faster than QEMU accepts frames, the owner should avoid unbounded queues/fail closed via existing bounded command/completion limits; do not introduce nested runtimes or a parallel QEMU writer path that wra-ynx7 would need to delete.

**2026-05-18T09:02:36Z**

Started after `wra-57z4` closure to resolve whether this follow-up still represents needed implementation. Audit finding: its original premise is stale in the current tree. The generic established-session ByteIo command/worker primitives mentioned in older `wra-nui7` notes were explicitly deleted by `wra-f762` because they had no production caller. Current production vmnet uses one Tokio owner loop with fd readiness snapshots for host-ingress/upstream sessions, bounded host-ingress/proxy per-session buffers and per-owner-pass pump limits, bounded DNS/TCP-connect service tasks, owner-side smoltcp/QEMU writes, and centralized closed-session reaping.

Cleanup direction: strongly prefer superseding this ticket rather than reintroducing ByteIo workers. Re-adding established-session workers now would add a second readiness/backpressure path immediately before `wra-ynx7` deletes transitional scaffolding. Next step is to document the supersession on `wra-ynx7` and close `wra-jenv` as superseded if no live correctness gap is found.

**2026-05-18T09:02:49Z**

Supersession decision: close this ticket without reintroducing established-session byte-IO workers. `wra-f762` already deleted the unused generic ByteIo surface, and the current production path has one coherent readiness/backpressure model: Tokio owner selects QEMU/service/fd/timer events; host/upstream session file descriptors are registered from `HostIngressBridge`/`TcpProxyBridge` interests; per-session buffers and per-owner-pass drains are bounded by `wra-57z4`; QEMU writes stay owner-side with `write_frame_async` backpressure; reaping is centralized by `wra-e9sr`. This is a smaller and cleaner architecture than moving established sockets into another worker layer.

Validation evidence for the superseding path: `wra-f762` required validation passed after deleting ByteIo and routing production DNS/TCP-connect service IO to Tokio; `wra-bbgh`/`wra-e9sr`/`wra-57z4` required validation passed most recently at `/tmp/pi-bash-22b3f93b38d6ca41.log` after the final buffer/drain cap. If future focused evidence shows owner-driven established-session IO starves under live load, file a new narrowly scoped performance bug rather than reopening this transitional worker plan.
