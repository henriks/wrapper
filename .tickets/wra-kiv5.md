---
id: wra-kiv5
status: closed
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


## Notes

**2026-05-16T17:08:54Z**

Started DNS/connect async-service IO slice after wra-t2uv closure. First code slice split DNS handling into synchronous parse/policy planning plus explicit upstream completion: DnsProxy::plan_udp_payload returns immediate malformed/blocked results or a DnsForwardRequest, and complete_forward maps worker results back to existing Allowed/UpstreamFailure responses. Existing handle_udp_payload behavior is preserved. Focused validation passed: cargo fmt; cargo test --manifest-path vm-frontend/Cargo.toml dns_proxy --offline.

**2026-05-16T17:11:12Z**

Added typed vmnet service command/completion envelopes for DNS lookup and TCP connect in vmnet_service_io.rs. Commands/completions carry owner-issued VmnetServiceToken and work kind while keeping DNS response frame construction and TCP close/reset application owner-side. Focused validation passed: cargo fmt; cargo test ... vmnet_service_io --offline; cargo test ... dns_proxy --offline.

**2026-05-16T17:13:23Z**

Added VmnetGateway DNS planning/completion hook: plan_dns_frame returns immediate owner-side DNS result or VmnetPendingDnsQuery carrying response context + DnsForwardRequest; complete_pending_dns_query applies service result back to DNS log/guest frame on owner path. Existing handle_guest_frame still behaves synchronously for now. Regression proves a gateway can plan allowed DNS without calling upstream and later synthesize SERVFAIL frame from a completion. Focused validation passed: cargo fmt; cargo test ... dns_query_can_be_planned --offline; cargo test ... vmnet_gateway::tests::dns_query --offline.

**2026-05-16T17:15:31Z**

Made upstream connect failures guest-visible in the current synchronous TcpProxyBridge path: connector errors now close the guest TCP session, carry close frames on TcpProxyEvent::ConnectFailed, and vmnet_runtime writes those frames like upstream payload frames. This preserves behavior for future async connect completions: failed completions must be applied owner-side and emit close/reset frames. Focused validation passed: cargo fmt; cargo test ... upstream_connect_failure_is_reported_without_creating_session --offline; cargo test ... tcp_proxy --offline; cargo test ... vmnet_runtime --offline.

**2026-05-16T17:18:23Z**

Split TCP connect setup in TcpProxyBridge into plan_connect (policy/TLS preparation, no connector IO) and complete_connect (owner-side success insertion or failure close frames). Existing process_gateway still performs synchronous connect for now, but the seam matches the async TcpConnect command/completion model. Focused validation passed: cargo fmt; cargo test ... connect_can_be_planned --offline; cargo test ... tcp_proxy --offline; cargo test ... vmnet_runtime --offline.

**2026-05-16T17:20:14Z**

Added VmnetGateway::handle_guest_frame_with_deferred_dns, which returns Forward(VmnetPendingDnsQuery) for allowed DNS without calling upstream while continuing to handle unrelated non-DNS frames synchronously on the owner. Regression uses a panic DNS upstream to prove deferred DNS does not block a subsequent TCP SYN from being accepted. Focused validation passed: cargo fmt; cargo test ... deferred_dns_query_does_not_block_unrelated_tcp_syn --offline; cargo test ... vmnet_gateway --offline; cargo test ... vmnet_runtime --offline.

**2026-05-16T17:23:34Z**

Added owner token allocation and tied pending DNS/connect work to typed service commands. VmnetServiceTokenSource now allocates monotonic owner tokens; VmnetPendingDnsQuery::service_command and TcpProxyPendingConnect::service_command build DNS/TCP service commands while keeping owner-side context/pending state local for later completion. Focused validation passed: cargo fmt; cargo test ... vmnet_service_io --offline; cargo test ... pending_dns_query_builds_service_command --offline; cargo test ... connect_can_be_planned --offline.

**2026-05-16T17:25:16Z**

Added VmnetServicePending<T>, an owner-side pending map keyed by VmnetServiceToken with duplicate-token rejection and remove-on-completion semantics. This prepares DNS/connect completion application without losing owner context. Also cleaned transitional service_command warning points while worker wiring is still pending. Focused validation passed: cargo fmt; cargo test ... vmnet_service_io --offline; cargo test ... pending_dns_query_builds_service_command --offline; cargo test ... connect_can_be_planned --offline.

**2026-05-16T17:27:51Z**

Added pollable service-IO wakeup plumbing without adopting Tokio: VmnetServiceWakeup provides a nonblocking UnixStream wake pipe, VmnetEventSource::ServiceIo is registered/dispatchable by RuntimePoller, and RuntimeReadyDispatch now marks service_io readiness. This prepares worker completion wakeups so DNS/connect completions do not wait for unrelated QEMU/socket events. Focused validation passed: cargo fmt; cargo test ... vmnet_service_io --offline; cargo test ... runtime_ready_dispatch_marks_service_io_wakeup --offline.

**2026-05-16T17:29:34Z**

Added remove_for_completion on VmnetServicePending<T> so owner completion application can atomically match a service completion token to pending DNS/connect context and ignore stale/unknown completions. Focused vmnet validation passed: cargo fmt; cargo test ... vmnet_service_io --offline; cargo test ... vmnet --offline (61 passed, 2 ignored stress tests as before, plus bin vmnet config tests).

**2026-05-16T17:31:29Z**

Added VmnetServiceOwner<Pending, C>, combining owner token allocation, pending context, bounded command queue, completion queue, and completion-to-pending matching. Tests cover successful DNS-style submission/completion matching and explicit full-command-queue rejection without inserting pending context. Focused vmnet validation passed: cargo fmt; cargo test ... vmnet_service_io --offline; cargo test ... vmnet --offline (63 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T17:34:05Z**

Added bounded cancel semantics to VmnetServiceOwner: cancel_pending removes pending context only if it can enqueue a Cancel command; if the command queue is full, pending context is restored and the full error reports the rejected cancel command. Focused vmnet validation passed: cargo fmt; cargo test ... vmnet_service_io --offline; cargo test ... vmnet --offline (65 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T17:39:04Z**

Added runtime-level DNS service helper functions: handle_guest_frame_with_dns_service queues allowed deferred DNS through VmnetServiceOwner or fails closed with SERVFAIL on full command queue; apply_dns_service_completion matches owner pending context and synthesizes guest DNS response frames on completion. Tests cover queued DNS completion and full-queue SERVFAIL while preserving pending state. Focused validation passed: cargo fmt; cargo test ... dns_service_helper --offline; cargo test ... vmnet --offline (67 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T17:42:12Z**

Added DNS service executor primitive in vmnet_service_io: execute_dns_service_command maps DnsLookup commands to typed DNS completions via DnsUpstream, converts DNS cancels to stale-safe Cancelled completions without upstream calls, and leaves non-DNS commands unsupported for other workers. Focused validation passed: cargo fmt; cargo test ... vmnet_service_io --offline (18 tests); cargo test ... vmnet --offline (70 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T17:44:13Z**

Added a DNS service queue-step primitive: run_dns_service_owner_step consumes one bounded service command, executes DNS work through the DNS executor, and enqueues a bounded completion or reports completion-queue-full without touching owner pending state. Tests cover completed step, full completion queue, and idle/no-upstream behavior. Focused validation passed: cargo fmt; cargo test ... vmnet_service_io --offline (21 tests); cargo test ... vmnet --offline (73 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T17:46:26Z**

Added VmnetServiceOwner::submit_to for external bounded sinks. This keeps token allocation and pending-state insertion owner-side, but lets a future runtime worker channel be the actual command queue; if the sink rejects, pending context is not inserted and the command/pending/error are returned. Focused validation passed: cargo fmt; cargo test ... vmnet_service_io --offline (23 tests); cargo test ... vmnet --offline (75 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T17:48:59Z**

Reflection/update: DNS now has both owner-side and worker-side primitives, including a spawned bounded DNS worker backed by std sync_channel plus VmnetServiceWakeup notification. Tests prove worker completion wakes the owner and zero-capacity limits are rejected. This still is not wired into serve_vmnet_gateway; remaining risk is production-loop integration, draining completion queues without duplicate/stale pending state, and deciding full completion queue/shutdown behavior before closing the ticket. Focused validation passed: cargo fmt; cargo test ... vmnet_service_io --offline (25 tests); cargo test ... vmnet --offline (77 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T17:51:29Z**

Added runtime-side DNS worker completion drain helper: drain_dns_worker_completions drains a spawned DNS worker's completion receiver, matches completions back through VmnetServiceOwner pending state, and applies DNS response frame synthesis owner-side. Test covers deferred DNS -> external worker submission -> service wakeup -> completion drain -> guest-visible SERVFAIL response without using gateway's synchronous upstream. Focused validation passed: cargo fmt; cargo test ... dns_worker_completion_drain --offline; cargo test ... vmnet --offline (78 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T17:54:09Z**

Wired DNS service primitives into serve_vmnet_gateway: runtime now creates a VmnetServiceWakeup, spawned bounded DNS worker, and VmnetServiceOwner; registers ServiceIo with RuntimePoller; guest DNS frames are submitted via external bounded worker sink; ServiceIo readiness drains worker completions and writes owner-synthesized guest frames/log events. Synchronous gateway DNS remains for non-serve helper paths. Focused validation passed: cargo fmt; cargo test ... vmnet_runtime --offline (12 tests); cargo test ... vmnet --offline (78 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T17:57:00Z**

Added delayed/blocked DNS worker regression: a DNS query queued to a worker whose upstream is blocked leaves pending DNS state, while an unrelated TCP SYN still progresses immediately on the vmnet owner and produces guest frames; after releasing DNS, ServiceIo wakeup drains completion and clears pending state. Focused validation passed: cargo fmt; cargo test ... pending_dns_worker_query_does_not_block_unrelated_tcp_syn --offline; cargo test ... vmnet --offline (79 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T17:58:46Z**

Added TCP connect service executor primitive in vmnet_service_io: execute_tcp_connect_service_command maps TcpConnect commands through TcpUpstreamConnector into typed TcpConnect completions, reports connector failures, converts TCP cancel commands without touching the connector, and leaves DNS commands for the DNS worker. Focused validation passed: cargo fmt; cargo test ... tcp_connect_service_executor --offline (4 tests); cargo test ... vmnet --offline (83 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T18:00:28Z**

Added spawned TCP connect worker primitive using the shared VmnetServiceWorkerHandle shape. spawn_tcp_connect_service_worker uses bounded sync_channel queues, executes typed TcpConnect commands through TcpUpstreamConnector, and wakes the owner via VmnetServiceWakeup when completions are ready. Also generalized DNS/TCP worker handles via VmnetServiceWorkerHandle aliases to avoid duplicating handle code. Focused validation passed: cargo fmt; cargo test ... spawned_tcp_connect_worker --offline (2 tests); cargo test ... vmnet_service_io --offline (31 tests); cargo test ... vmnet --offline (85 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T18:04:22Z**

Added owner-side TCP connect completion application helper in vmnet_runtime: apply_tcp_connect_service_completion removes pending connect context by token and routes success/failure through TcpProxyBridge::complete_connect. Tests cover success inserting a live proxy session and failure producing guest close/reset frames while clearing pending state. Focused validation passed: cargo fmt; cargo test ... tcp_connect_completion --offline (2 tests); cargo test ... vmnet_runtime --offline (15 tests); cargo test ... vmnet --offline (87 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T18:06:12Z**

Added TCP connect worker completion drain helper: drain_tcp_connect_worker_completions drains spawned TCP worker completions, applies each via apply_tcp_connect_service_completion on the owner, returns events, and reports worker disconnect. Test covers pending connect submission to spawned worker, ServiceIo wakeup, completion drain, owner-side session insertion, and pending-state cleanup. Focused validation passed: cargo fmt; cargo test ... tcp_connect_worker_completion_drain --offline; cargo test ... vmnet_runtime --offline (16 tests); cargo test ... vmnet --offline (88 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T18:09:20Z**

Added TCP connect worker submission helper and pending tracking in TcpProxyBridge. Runtime helper submit_tcp_connects_to_worker plans established guest sessions without proxy sessions, submits pending connects to the bounded worker sink, marks accepted handles as pending, and uses complete_connect fail-closed behavior if the sink rejects. TcpProxyBridge now skips synchronous connector calls for pending handles, preventing duplicate sync/async connects. Test covers worker submission marking pending and process_gateway skipping a panic synchronous connector. Focused validation passed: cargo fmt; cargo test ... tcp_connect_worker_submission --offline; cargo test ... vmnet_runtime --offline (17 tests); cargo test ... vmnet --offline (89 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T18:11:46Z**

Wired TCP connect worker into serve_vmnet_gateway. The runtime now shares one VmnetServiceWakeup between DNS and TCP connect workers, spawns a bounded TCP connect worker from the mapped connector, owns VmnetServiceOwner<TcpProxyPendingConnect, TcpStream>, submits pending TCP connects to the worker before proxy pumping, drains connect completions on ServiceIo readiness, writes any guest close/reset frames from connect failures, and logs resulting proxy events. Refactored proxy guest-frame writing into write_proxy_event_guest_frames. Focused validation passed: cargo fmt; cargo test ... vmnet_runtime --offline (17 tests); cargo test ... vmnet --offline (89 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T18:17:05Z**

Added slow TCP connect regression and worker full-completion shutdown hardening. Regression  uses a blocked TCP connect worker to prove the vmnet owner can still accept an unrelated TCP SYN while the first connect is pending, then drains the completion owner-side. Spawned DNS/TCP workers now use nonblocking  for completions and exit on full/disconnected completion queues instead of blocking indefinitely;  covers shutdown behavior when a completion queue is full. Focused validation passed: cargo fmt; cargo test ... slow_tcp_connect_worker_does_not_block_unrelated_tcp_syn --offline; cargo test ... vmnet_service_io --offline (32 tests); cargo test ... vmnet_runtime --offline (18 tests); cargo test ... vmnet --offline (91 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T18:17:14Z**

Correction to prior note: added slow TCP connect regression named slow_tcp_connect_worker_does_not_block_unrelated_tcp_syn. Spawned DNS/TCP workers now use nonblocking completion try_send and exit on full/disconnected completion queues instead of blocking indefinitely. Added spawned_tcp_connect_worker_shutdown_does_not_block_on_full_completion_queue to cover shutdown when completion queue is full. Focused validation passed: cargo fmt; cargo test --manifest-path vm-frontend/Cargo.toml slow_tcp_connect_worker_does_not_block_unrelated_tcp_syn --offline; cargo test --manifest-path vm-frontend/Cargo.toml vmnet_service_io --offline (32 tests); cargo test --manifest-path vm-frontend/Cargo.toml vmnet_runtime --offline (18 tests); cargo test --manifest-path vm-frontend/Cargo.toml vmnet --offline (91 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T18:22:43Z**

Finalized worker disconnect/cleanup semantics for DNS/connect service IO. VmnetServicePending/VmnetServiceOwner can now drain pending owner context for fail-closed cleanup. DNS worker disconnect drains produce SERVFAIL guest responses for pending DNS; TCP connect worker disconnect drains close/reset pending guest TCP sessions through TcpProxyBridge::complete_connect. Timeout review: worker operations remain bounded by existing DNS UdpDnsUpstream timeout and StdTcpConnector connect_timeout; cancellation remains token-based/best-effort for queued work and stale completions, while in-flight blocking calls rely on those operation deadlines rather than moving smoltcp ownership or adding Tokio. Required validation passed: ./vm-frontend/validate.sh required (included live-setup-tools scenarios); no appliance rebuild required.
