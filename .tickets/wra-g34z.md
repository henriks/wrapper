---
id: wra-g34z
status: closed
deps: []
links: [wra-ynx7, wra-rh21, wra-vl1o, wra-p06p]
created: 2026-05-18T10:36:19Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-emj5
tags: [cleanup, vmnet, tcp, tls]
---
# Consolidate vmnet event selection and bounded stream buffers

After wra-ynx7, vmnet no longer has the old sync scaffolding, but vmnet_runtime.rs still has multiple async owner event selection variants, while tcp_proxy.rs and host_ingress.rs maintain similar bounded read/write buffering helpers. Future TLS/session fixes should not add another parallel path.

## Design

Audit next_async_qemu_or_* style event selection in vmnet_runtime.rs and bounded read/write helpers in tcp_proxy.rs and host_ingress.rs. Extract or merge only where it deletes real duplication and keeps smoltcp/VmnetGateway single-owner semantics. Coordinate with wra-vl1o and wra-rh21 so TLS fatal-session fixes and cert-cache bounding do not duplicate buffer/session cleanup machinery.

## Acceptance Criteria

vmnet owner event selection is represented by one coherent helper/pattern; duplicated bounded stream-buffer logic between TCP proxy and host ingress is reduced or explicitly justified; TLS/session tickets can build on the shared path; vmnet tests and required validation are recorded before close.


## Notes

**2026-05-18T11:43:51Z**

Iteration 11 start: choosing this non-appliance cleanup while wra-b9b1 waits for post-change appliance rebuild/live validation. Audit target is vm-frontend/src/vmnet_runtime.rs event-selection helpers plus bounded stream buffer duplication in vm-frontend/src/tcp_proxy.rs and vm-frontend/src/host_ingress.rs. Avoid appliance changes for this ticket.

**2026-05-18T11:51:38Z**

Iteration 12 progress: consolidated vmnet async owner event selection around the fd snapshot helper. Removed the older test-only single-fd owner selector and qemu-or-timer/qemu-or-fd helper variants from vmnet_runtime.rs; updated owner-event tests to call next_async_owner_event_with_fd_snapshot with empty or singleton registration snapshots, matching the production pattern. Verification: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --lib async_owner_event -- --nocapture; full ./vm-frontend/validate.sh required also passed as part of wra-b9b1 validation. Remaining wra-g34z work: audit/reduce bounded stream-buffer duplication between tcp_proxy.rs and host_ingress.rs.

**2026-05-18T12:00:25Z**

Iteration 13 completion: reduced bounded stream-buffer duplication between tcp_proxy.rs and host_ingress.rs by adding vm-frontend/src/stream_buffer.rs with shared pending-buffer limit checking/extension, nonblocking chunk reads, and best-effort pending writes. host_ingress now uses shared extend/read/write helpers for pending_guest_write and pending_host_write; tcp_proxy now uses shared extend/write helpers for pending upstream bytes, pending upstream plaintext, and pending guest bytes. Combined with iteration 12, vmnet owner event selection is represented by next_async_owner_event_with_fd_snapshot rather than parallel single-fd/timer helper variants. Validation: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo check --manifest-path vm-frontend/Cargo.toml --lib; cargo test --manifest-path vm-frontend/Cargo.toml --lib tcp_proxy::tests; cargo test --manifest-path vm-frontend/Cargo.toml --lib host_ingress::tests; cargo test --manifest-path vm-frontend/Cargo.toml --lib vmnet_runtime::tests::async_owner_event; ./vm-frontend/validate.sh required passed with timeout 300s. No appliance inputs touched.
