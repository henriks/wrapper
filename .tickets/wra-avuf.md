---
id: wra-avuf
status: closed
deps: [wra-f762]
links: []
created: 2026-05-17T19:41:23Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, vmnet]
---
# Replace vmnet RuntimePoller owner loop with Tokio readiness

Follow-up split from wra-f762 after production DNS/TCP service I/O moved to Tokio tasks. The remaining wra-f762 acceptance gap is the owner loop: vmnet_runtime::serve_vmnet_gateway still uses vmnet_poller::RuntimePoller (mio/raw-fd readiness) plus VmnetServiceWakeup to drive QEMU stream frames, host listener accepts, host ingress sessions, upstream proxy sessions, service completions, and smoltcp timer polling. This ticket should replace that sync poller owner loop with a Tokio-owned vmnet actor/select boundary while preserving one owner for VmnetCore/VmnetGateway/smoltcp state; do not introduce Arc<Mutex<_>> around gateway/core state.

## Design

Start by adding async I/O boundary primitives for QEMU frame I/O and readiness sources, then migrate serve_vmnet_gateway or add an async serve variant used by both CLI/supervisor paths. Dynamic host/upstream session registration must be modeled without reintroducing a bespoke mio poller. Service I/O completions already use bounded tokio::sync::mpsc and spawn_blocking (wra-f762 iterations 9-11), so reuse those handles or simplify VmnetServiceWakeup away. Keep composed-fs/vhost filesystem work unaffected.

## Acceptance Criteria

vmnet production path runs without vmnet_poller::RuntimePoller/mio raw-fd registration and without VmnetServiceWakeup; vmnet owner state remains single-owner; async tests cover fake QEMU stream reads/writes, DNS and TCP-connect completions, dynamic host-ingress/upstream session readiness, shutdown/EOF, and smoltcp timer behavior; ./vm-frontend/validate.sh required passes before closure.


## Notes

**2026-05-17T19:42:58Z**

Starting after wra-f762 closed/split. First slice will be small and additive: add async QEMU frame I/O primitives/tests in vmnet_stream so a future Tokio owner loop can read/write frame streams without immediately replacing production RuntimePoller. Keep VmnetCore/smoltcp single-owner and production routing unchanged in this slice.

**2026-05-17T19:44:20Z**

Iteration 13 first additive async owner-loop seam: added async QEMU frame I/O primitives to vmnet_stream. QemuFrameIo now has read_frame_async and write_frame_async for Tokio AsyncRead/AsyncWrite streams, preserving the existing big-endian length-prefix framing, max-frame validation, buffered partial-frame handling, EOF/truncated-length/truncated-frame errors, and synchronous APIs. Added Tokio duplex tests for async decode, encode, and truncated length handling. Existing proptest coverage still exercises arbitrary frame bytes/lengths through the shared buffered parser. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_stream -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T19:46:53Z**

Iteration 14 added a small async owner-loop harness in vmnet_runtime on top of the async QEMU frame I/O seam. New handle_next_async_qemu_frame reads one async guest frame and applies it on the single owner VmnetCore; write_guest_frames_async writes owner-produced frames back through async QemuFrameIo while preserving stats/pcap behavior. Added Tokio duplex tests proving a guest TCP SYN can be handled and a response frame written back, plus clean EOF handling. Production serve_vmnet_gateway still uses RuntimePoller. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_qemu_frame_harness -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_stream -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T19:49:01Z**

Iteration 15 extended the async vmnet owner-loop harness to cover a non-QEMU wake source without RuntimePoller/VmnetServiceWakeup. Added next_async_qemu_or_dns_event, a tokio::select! seam over async QEMU frame reads and DNS worker completions. Tests prove the harness can receive a DNS completion directly from the bounded Tokio service task and apply it on the owner side, and can receive a QEMU frame without the poller. Production serve_vmnet_gateway remains unchanged. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_select_harness -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T19:50:34Z**

Iteration 16 reflection/harness extension: async owner-loop harness now covers QEMU frames, DNS completions, and TCP-connect completions without RuntimePoller/VmnetServiceWakeup. Added VmnetAsyncQemuTcpEvent and next_async_qemu_or_tcp_event using tokio::select! over async QEMU frame reads and bounded Tokio TCP worker completions. New test proves a TCP connect completion is received directly and applied on owner-side TcpProxyBridge state. Production serve_vmnet_gateway remains unchanged. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_select_harness -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T19:52:44Z**

Iteration 17 added timer wake coverage to the async owner-loop harness. New VmnetAsyncQemuTimerEvent and next_async_qemu_or_timer_event select over async QEMU frame reads and tokio::time::sleep, giving the future owner actor a no-RuntimePoller path for smoltcp timer wakeups. Tests prove a timer event fires without a poller and that a ready QEMU frame wins over a later timer. Production serve_vmnet_gateway remains unchanged. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_select_harness -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T19:54:47Z**

Iteration 18 added dynamic session readiness coverage to the async owner-loop harness. New VmnetAsyncFdReadyEvent, BorrowedRawFd, and next_async_qemu_or_fd_readable use tokio::io::unix::AsyncFd to select over async QEMU frames and borrowed host/upstream session fd readability without RuntimePoller. Tests prove HostSession and UpstreamSession readiness can be reported without mio/raw-fd registration in RuntimePoller. Production serve_vmnet_gateway remains unchanged. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_fd_harness -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T19:58:18Z**

Iteration 19 completed the remaining host-listener readiness harness coverage. HostIngressListenerSet::local_addrs is now pub(crate) for tests, and vmnet_runtime has async_fd_harness_reports_host_listener_accept_readiness_without_poller: it binds a real nonblocking host listener on port 0, connects to it, observes VmnetEventSource::HostListener readiness via tokio::io::unix::AsyncFd/next_async_qemu_or_fd_readable, then accepts through HostIngressListenerSet to prove accept readiness is actionable without RuntimePoller. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_fd_harness -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check. Production serve_vmnet_gateway is still unchanged; async harness now covers QEMU, DNS/TCP service completions, timers, host listeners, host sessions, and upstream sessions.

**2026-05-17T20:02:19Z**

Iteration 20 inspected production serve_vmnet_gateway and found the main production migration blocker: replacing RuntimePoller means the QEMU stream must become a Tokio AsyncRead/AsyncWrite boundary, but existing proxy/host-ingress pump helpers were tied to QemuFrameIo<T: std::io::Read + Write> for guest-frame writes. Added the first production-prep seam: async proxy and host-ingress guest-frame writers plus pump_proxy_ready_async and pump_host_ingress_ready_async, preserving synchronous single-owner gateway/proxy/host state while allowing guest-frame writes to a Tokio stream. Added async_event_guest_frame_writers_use_tokio_stream to prove proxy and host-ingress events write length-prefixed frames through Tokio duplex/QemuFrameIo::write_frame_async. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_event_guest_frame_writers -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check. Production serve_vmnet_gateway remains unchanged.

**2026-05-17T20:03:42Z**

Iteration 21 reflection: wra-avuf now has harness coverage for all RuntimePoller wake classes and async guest-frame write seams for proxy/host-ingress events. Working well: keeping smoltcp/VmnetCore/proxy/host-ingress single-owner while moving only byte-stream readiness and service completions toward Tokio has produced small testable slices. Blocking/risk: production serve_vmnet_gateway still uses RuntimePoller/VmnetServiceWakeup; the remaining migration must avoid duplicating a second owner loop or adding shared mutable state. Approach adjustment: introduce the async production wrapper from the QEMU stream boundary inward: sync public wrapper can own/block_on a Tokio inner, QEMU socket accept/read/write should become Tokio, service completions should be direct channel selects, and host/proxy fd readiness can use AsyncFd without moving pump execution off-owner. Iteration 21 added VmnetStreamEndpoint::accept_one_tokio using AsyncFd over the existing UnixListener fd plus accept_ready conversion to tokio::net::UnixStream, with endpoint_accepts_tokio_qemu_stream proving async accept and frame decode. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline endpoint_accepts_tokio_qemu_stream -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_stream -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T20:05:40Z**

Iteration 22 chose and wired the minimal production wrapper shape: public serve_vmnet_gateway now owns an agentvm-vmnet-runtime multi-thread Tokio runtime and block_on's private serve_vmnet_gateway_async. The async inner currently uses tokio::task::block_in_place to run the existing single-owner blocking RuntimePoller loop (renamed serve_vmnet_gateway_blocking_poller), so behavior remains staged while production now has a Tokio orchestration boundary to convert inward. RuntimePoller/VmnetServiceWakeup are still present in the blocking inner. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_stream -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T20:06:54Z**

Iteration 23 moved production vmnet service tasks onto the new owner runtime. The blocking-poller inner no longer constructs a nested agentvm-vmnet-service-io Tokio runtime; it enters tokio::runtime::Handle::current() from the public serve_vmnet_gateway runtime and spawns DNS/TCP-connect tasks there. RuntimePoller/VmnetServiceWakeup remain for completion notification while the blocking inner is still in place, but runtime ownership is now single-layered and ready for direct channel selection in the async inner. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_service_io -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T20:08:44Z**

Iteration 24 added the direct service-completion selection seam needed to remove VmnetServiceWakeup from the async owner loop. New VmnetAsyncServiceCompletionEvent and next_async_service_completion select directly over DNS and TCP-connect worker completion channels with tokio::select!, without RuntimePoller or wakeup fd. Added tests for DNS and TCP completions proving owner-side application works through direct channel selection and pending state is cleared. Production still uses RuntimePoller/VmnetServiceWakeup in the blocking inner; this slice establishes the no-wakeup service event shape for the next production migration. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_service_completion_select -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T20:10:43Z**

Iteration 25 added owner-side application for direct async service-completion events. New VmnetAsyncServiceCompletionApply and apply_async_service_completion_event consume VmnetAsyncServiceCompletionEvent, apply DNS/TCP completions on the single owner side, write resulting guest frames through async QemuFrameIo, and return gateway/proxy events plus disconnect flags/stats. Added async_service_completion_apply_writes_dns_guest_frame_without_wakeup_fd, proving a direct DNS completion writes a length-prefixed response frame without RuntimePoller/VmnetServiceWakeup and clears pending DNS state. Production still uses the blocking poller/wakeup bridge; this slice prepares the code that can replace the service_io dispatch branch in an async inner. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_service_completion -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T20:12:24Z**

Iteration 26 reflection: wra-avuf has now proven async seams for QEMU stream accept/read/write, DNS/TCP service completion wait/apply, smoltcp timer wakeups, host listener readiness, host session readiness, upstream session readiness, and async proxy/host-ingress guest-frame writes. Working well: keeping VmnetCore/smoltcp/proxy/host-ingress synchronous and single-owner while moving only orchestration and byte-stream boundaries to Tokio continues to produce small, testable slices. Blocking/risk: production still delegates from the async wrapper to serve_vmnet_gateway_blocking_poller, so RuntimePoller and VmnetServiceWakeup still gate live behavior. Approach adjustment: avoid adding another parallel full loop; collapse the separate harness shapes into one owner-event wait/apply path and then migrate the existing loop branches into it. Iteration 26 added VmnetAsyncOwnerEvent and next_async_owner_event, selecting over QEMU frame reads, direct DNS/TCP service completions, and timer sleep without RuntimePoller/VmnetServiceWakeup. Tests prove service completion beats a long timer and timer fires without poller/wakeup. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_owner_event_select -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T20:15:02Z**

Iteration 27 extended the unified owner-event seam to fd readiness. VmnetAsyncOwnerEvent now includes FdReadable(VmnetEventSource), and next_async_owner_event accepts an optional fd readiness source backed by tokio::io::unix::AsyncFd while still selecting over QEMU frames, direct service completions, and timers. Added tests proving the unified event path reports HostListener readiness against a real HostIngressListenerSet plus HostSession and UpstreamSession readiness using nonblocking UnixStream pairs, all without RuntimePoller/VmnetServiceWakeup. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_owner_event_select -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check. Production still delegates to serve_vmnet_gateway_blocking_poller.

**2026-05-17T20:18:23Z**

Iteration 28 added the first fd-registration snapshot and richer fd interest model for the async owner-event spine. Added VmnetAsyncFdInterest and VmnetAsyncFdRegistration plus snapshot_async_fd_registrations, which snapshots host listeners, host-ingress sessions, and upstream proxy sessions into fd registrations without RuntimePoller. next_async_owner_event now accepts an optional VmnetAsyncFdRegistration and can wait for readable or writable readiness via AsyncFd; VmnetAsyncOwnerEvent now reports FdReady { source, readable, writable }. Added async_fd_snapshot_includes_host_listeners_without_poller_registration and updated owner-event fd tests for the richer event. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_fd_snapshot -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline async_owner_event_select -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check. Production still delegates to serve_vmnet_gateway_blocking_poller.

**2026-05-17T20:20:08Z**

Iteration 29 added the deterministic fd scheduling policy for the async owner-event spine. Added choose_async_fd_registration plus async_fd_registration_poll_ready: from an fd snapshot, it scans from a cursor for an immediately ready registration using a zero-timeout libc::poll check, prefers that ready fd, and otherwise round-robins the next registration while advancing the cursor. Added tests proving a ready fd later in the snapshot is preferred and that fallback round-robins when no fd is ready. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_fd_registration_choice -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline async_owner_event_select -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check. Production still delegates to serve_vmnet_gateway_blocking_poller.

**2026-05-17T20:24:11Z**

Iteration 30 added an async-owner production skeleton but intentionally did not wire it as the live path. New serve_vmnet_gateway_async_owner mirrors the production initialization and loop shape using Tokio QEMU accept/read/write, direct non-notifier DNS/TCP service tasks on the owner runtime, the unified owner-event wait spine, async fd snapshots/selection, async service completion application, and async proxy/host-ingress guest-frame writes. Public serve_vmnet_gateway still calls serve_vmnet_gateway_async, which continues to block_in_place into serve_vmnet_gateway_blocking_poller; RuntimePoller/VmnetServiceWakeup still gate live behavior. Reason for not flipping yet: the current fd wait path still waits on one selected AsyncFd at a time, so production needs multi-fd waiting or another safe strategy before removing RuntimePoller. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_fd_registration_choice -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T20:26:39Z**

Iteration 31 reflection: wra-avuf now has a non-live async-owner production skeleton and unified owner-event path. Working well: byte-stream/service/timer/fd seams are all tested without moving VmnetCore/smoltcp/proxy/host-ingress off the owner. Blocking/risk: live production still uses RuntimePoller/VmnetServiceWakeup, and flipping requires safe waiting across the whole fd snapshot. Approach adjustment: solve multi-fd readiness in the unified event helper rather than relying on one selected AsyncFd. Iteration 31 added wait_async_fd_registrations and next_async_owner_event_with_fd_snapshot. wait_async_fd_registrations builds AsyncFd wrappers for the fd snapshot, polls all readable/writable interests in one future, and advances the cursor on readiness. serve_vmnet_gateway_async_owner now uses the snapshot-aware event helper instead of one selected registration. Added async_owner_event_snapshot_wait_receives_later_ready_fd proving a later-ready fd in the snapshot wakes the unified owner event even when an earlier fd is unready. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline async_owner_event_snapshot_wait -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check. Production still delegates to serve_vmnet_gateway_blocking_poller.

**2026-05-17T20:31:47Z**

Wired live vmnet gateway to the Tokio async owner loop: serve_vmnet_gateway_async now awaits serve_vmnet_gateway_async_owner instead of block_in_place delegating to the RuntimePoller loop. Focused vmnet_runtime/vmnet_stream/vmnet_service_io tests passed, then ./vm-frontend/validate.sh required passed with live setup-tool scenarios. Old blocking RuntimePoller fallback remains only as dead rollback/reference code for a deletion cleanup slice.

**2026-05-17T20:36:15Z**

Deletion cleanup after async owner loop flip: removed the dead serve_vmnet_gateway_blocking_poller fallback, RuntimeReadyDispatch, sync poller registration helpers/tests, and the now-unused VmnetServiceWakeup/notifier service-task bridge plus notifier tests. Production vmnet_runtime no longer references RuntimePoller or VmnetServiceWakeup. Focused validation passed: cargo fmt, vmnet_service_io tests, vmnet_runtime tests, plus vmnet_stream in the earlier focused run. Remaining cleanup before closure: move/remove the now-orphaned vmnet_poller module (VmnetEventSource is still imported from it) and run ./vm-frontend/validate.sh required.

**2026-05-17T20:39:36Z**

Final cleanup/validation for poller replacement: moved VmnetEventSource into vmnet_runtime, removed vmnet_poller from lib.rs, deleted vmnet_poller.rs, and confirmed no RuntimePoller/VmnetServiceWakeup/notifier bridge references remain. Focused validation passed for vmnet_runtime, vmnet_service_io, and vmnet_stream. ./vm-frontend/validate.sh required passed with live setup-tool scenarios after the cleanup.
