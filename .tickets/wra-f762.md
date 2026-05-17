---
id: wra-f762
status: in_progress
deps: [wra-73tn]
links: []
created: 2026-05-17T10:19:45Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, vmnet, deletion]
---
# Option 1: replace vmnet poller and service I/O with Tokio actor

Delete the hand-built vmnet runtime pieces once the core/I/O split is ready. vmnet_poller.rs duplicates Tokio readiness, and vmnet_service_io.rs recreates async primitives with sync channels, UnixStream wakeups, tokens, and worker threads.

## Design

Drive one owner vmnet actor with tokio::select over QEMU frame reads, host listener accepts, DNS/connect/session completion channels, smoltcp timer sleeps, and cancellation. Replace DNS and TCP connect workers with bounded tokio::sync::mpsc or oneshot replies and tokio::time::timeout. Remove VmnetServiceWakeup, RuntimePoller, raw fd registration, generic VmnetByteIo layers unless a real production caller remains, and old sync tick helpers after async tests replace them.

## Acceptance Criteria

vmnet runs without mio RuntimePoller or std::sync::mpsc service workers, backpressure and queue-full behavior remain tested, smoltcp state remains single-owner, and async integration tests cover fake QEMU streams, DNS/connect completion, host ingress, shutdown, and timer behavior.


## Notes

**2026-05-17T10:28:44Z**

Async-boundary refinement: replacing vmnet poller/service I/O with Tokio should not make the gateway core concurrent. Preserve one owner actor for smoltcp and frame ordering; use Tokio for edge readiness and bounded worker communication only.

**2026-05-17T19:07:22Z**

Starting in wra-xcvq-complete-epic iteration 1 while wra-662v Rust opt-in appliance validation is blocked on a privileged rebuild. First slice will be intentionally additive/mechanical: inspect vmnet_runtime/vmnet_poller/vmnet_service_io seams and add a small Tokio-facing actor boundary or test harness without moving VmnetGateway/VmnetCore state behind Arc<Mutex<_>> and without deleting the existing live-validated synchronous runtime yet.
