---
id: wra-f762
status: closed
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

**2026-05-17T19:09:47Z**

Iteration 1 first deletion slice: removed the unused spawned byte-I/O worker entrypoint from vmnet_service_io. There is no production caller for spawn_byte_io_service_worker/VmnetByteIoWorkerHandle; host-ingress and upstream session I/O are currently driven owner-side via existing bridge pump helpers, while production off-owner workers are DNS lookup and TCP connect. Deleted the worker alias/function and its worker-thread tests, keeping the lower-level byte-I/O command/executor tests for now as a narrower follow-up decision. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_service_io -- --nocapture.

**2026-05-17T19:13:07Z**

Iteration 2 deletion slice: removed the remaining generic byte-I/O service command/executor surface from vmnet_service_io. There are no production callers for VmnetServiceCommand::ByteIo / VmnetServiceCompletion::ByteIo, HostIngress/UpstreamSession service kinds, byte-I/O execution helpers, or their tests; host ingress and upstream proxy session I/O are still driven by the runtime bridge pump helpers. Vmnet service IO is now narrowed to the production off-owner work classes DNS lookup and TCP connect plus cancellation. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_service_io -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture.

**2026-05-17T19:15:40Z**

Iteration 3 first Tokio seam: added an additive Tokio-backed DNS service task in vmnet_service_io without routing production vmnet through it yet. New VmnetAsyncServiceWorkerHandle uses bounded tokio::sync::mpsc command/completion channels, runs blocking DnsUpstream::exchange through tokio::task::spawn_blocking so DNS work does not occupy core Tokio workers, skips unsupported commands, preserves bounded capacity validation, and requires no VmnetServiceWakeup fd. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_dns_service_task -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_service_io -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T19:18:52Z**

Iteration 4 added the matching Tokio TCP-connect service seam in vmnet_service_io. New spawn_tcp_connect_service_task uses bounded tokio mpsc queues and spawn_blocking around TcpUpstreamConnector::connect, with tests for successful completion, unsupported command skipping, cancellation, and capacity validation. Production vmnet routing is still unchanged. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_tcp_connect_service_task -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_service_io -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T19:21:19Z**

Iteration 5 added the owner-side async service adapter below production routing: VmnetServiceOwner::submit_to_async_worker submits through a bounded Tokio worker handle while preserving pending-state semantics on TrySendError, and drain_from_async_worker drains async completions back into the owner queue with disconnection reporting. Added DNS and TCP adapter tests plus a full-channel preservation test. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_owner_adapter -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_service_io -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T19:24:53Z**

Iteration 6 reflection/runtime-adapter step: added vmnet_runtime helper variants that use the async DNS/TCP service handles without moving smoltcp/VmnetCore ownership: handle_guest_frame_with_async_dns_worker, drain_async_dns_worker_completions, submit_tcp_connects_to_async_worker, and drain_async_tcp_connect_worker_completions. Added current-thread Tokio runtime tests proving async DNS failures and async TCP connect success are applied on the owner side. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_ -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_service_io -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T19:26:50Z**

Iteration 7 production seam review: production serve_vmnet_gateway is still a synchronous RuntimePoller loop running inside a blocking service thread/CLI path. The additive Tokio DNS/TCP workers cannot simply replace the wakeup-fd workers there without also giving the sync poller a completion notification or moving the loop to an async select boundary; otherwise completions could wait for unrelated fd/timer events. Added an async vmnet_runtime test for slow TCP connect proving the async worker path keeps blocking connect work on spawn_blocking and does not block unrelated owner-side TCP SYN handling. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline slow_async_tcp_connect_worker -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T19:29:17Z**

Iteration 8 added an explicit completion-notification bridge for async vmnet service tasks. New spawn_dns_service_task_with_notifier and spawn_tcp_connect_service_task_with_notifier preserve the existing bounded Tokio channel/spawn_blocking behavior while optionally notifying a VmnetServiceWakeup after a completion is enqueued, so the existing sync RuntimePoller can be woken if/when production runs these tasks on a driven Tokio runtime. Added poller-backed tests for DNS and TCP async completion wakeups. Production routing remains unchanged; the remaining production decision is where the Tokio runtime is owned/driven. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline notifies_pollable_wakeup -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_service_io -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T19:31:27Z**

Iteration 9 switched production vmnet DNS/TCP service I/O to the Tokio-backed service tasks. serve_vmnet_gateway now owns a small multi-thread Tokio runtime for vmnet service I/O, spawns DNS/TCP tasks with VmnetServiceWakeup notifiers, and the existing sync RuntimePoller loop drains async completions via the tested async helper paths. VmnetCore/smoltcp ownership remains single-threaded in the poller loop; blocking DNS/connect work remains on Tokio's blocking pool. default_dns_upstream now returns a Send+Sync trait object to satisfy the async task boundary. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_service_io -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T19:36:19Z**

Iteration 10 removed the old synchronous thread-worker service I/O surface now that production DNS/TCP service I/O uses Tokio tasks. Deleted VmnetServiceWorkerHandle, VmnetDnsWorkerHandle, VmnetTcpConnectWorkerHandle, spawn_dns_service_worker, spawn_tcp_connect_service_worker, and their mpsc/thread imports/tests from vmnet_service_io. Removed/adapted the remaining vmnet_runtime sync-worker helper/tests so runtime coverage now targets the async worker path. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_service_io -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T19:39:14Z**

Iteration 11 reflection/validation: broader required validation passed with the Tokio-backed production service-I/O path and old sync service-worker surface removed: ./vm-frontend/validate.sh required (exit 0). Important caveat: wra-f762 acceptance is not fully met yet because vmnet_runtime still uses the synchronous RuntimePoller/VmnetServiceWakeup bridge for the owner loop and service-completion wakeups. Service workers are now Tokio tasks and std::sync::mpsc service workers are gone; remaining work is the poller/actor portion or an explicit scope decision.

**2026-05-17T19:41:31Z**

Iteration 12 split decision after required validation: service-I/O migration is complete and validated, but the remaining RuntimePoller/owner-loop replacement is large enough to deserve its own explicit implementation ticket rather than hiding it inside the validated service-I/O slice. Created wra-avuf (Replace vmnet RuntimePoller owner loop with Tokio readiness), parent wra-xcvq, depending on wra-f762. wra-f762 should only close if its scope is treated as the service-I/O migration now superseded by wra-avuf for the poller/actor portion; otherwise keep it open until wra-avuf completes.

**2026-05-17T19:41:42Z**

Closing scope decision: service-I/O part of wra-f762 is implemented and live-capable required validation passed (./vm-frontend/validate.sh required, iteration 11). The remaining original poller/actor acceptance is explicitly split to wra-avuf, which depends on this ticket and carries the detailed RuntimePoller/VmnetServiceWakeup removal acceptance criteria. This avoids marking the poller replacement done prematurely while allowing the validated service-I/O migration ticket to close.
