---
id: wra-ynx7
status: closed
deps: [wra-bbgh, wra-wv0w, wra-57z4, wra-e9sr, wra-jenv]
links: [wra-73tn, wra-jenv, wra-e9sr, wra-f762, wra-57z4, wra-bbgh, wra-g34z]
created: 2026-05-18T05:36:42Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-9m5h
tags: [cleanup, vmnet, tokio]
---
# Remove vmnet transitional sync and dead-code scaffolding

The vmnet runtime still contains transitional sync helpers, allow(dead_code) islands, nested-runtime patterns, and readiness/buffer structures that were useful during the async migration but now make the runtime harder to reason about. After the active vmnet correctness tickets settle, remove obsolete sync/async duplicate code and keep a single owner-driven Tokio boundary for host service IO.

## Design

Coordinate with wra-bbgh, wra-57z4, wra-e9sr, and wra-jenv. Do not add another adapter layer. Delete unused QEMU stream helpers or move test-only pieces behind cfg(test). Collapse duplicated readiness and buffer accounting between HostIngress/TcpProxy paths where practical. If vmnet is exposed to the supervisor, expose the existing async service boundary directly rather than creating a nested runtime wrapper.

## Acceptance Criteria

Stale vmnet allow(dead_code) uses are gone or justified as test-only; no production nested Tokio runtime remains solely for vmnet bridging; active session byte IO has one coherent readiness/backpressure path; vmnet tests and required validation are recorded before close.


## Notes

**2026-05-18T06:48:30Z**

Dependency update: wra-wv0w is closed. Network policy IP/CIDR ranges are now parsed once into typed Ipv4Range values before vmnet/tcp runtime code; vmnet cleanup can assume runtime policy no longer carries raw IP range strings.

**2026-05-18T08:11:47Z**

Iteration 21 cleanup reflection: still blocked by vmnet correctness/backpressure prerequisites wra-bbgh, wra-57z4, wra-e9sr, and wra-jenv. Once those land, delete transitional sync/dead-code scaffolding around the single owner-driven Tokio vmnet path rather than adding more adapters.

**2026-05-18T08:19:36Z**

wra-bbgh audit during cleanup loop found QEMU all-or-nothing write_frame use appears confined to sync vmnet helper/test scaffolding, while production serve_vmnet_gateway uses the async owner path and write_frame_async().await. When wra-ynx7 becomes unblocked, consider deleting/quarantining run_qemu_stream_until_eof/run_qemu_stream_tick/pump_proxy_once-style sync frame-pump scaffolding instead of preserving it with a parallel write-queue implementation.

**2026-05-18T08:24:44Z**

wra-e9sr audit confirms the stale-session cleanup target is the central GuestTcpCore SocketSet/listener/host_connection slots; TcpProxyBridge and HostIngressBridge already remove many bridge-side sessions. When e9sr lands, wra-ynx7 can delete duplicated adapter cleanup assumptions and keep reaping in the single owner/core path.

**2026-05-18T09:00:14Z**

Cleanup prerequisite update from `wra-57z4`: tcp-proxy incomplete HTTP request buffering is now bounded and fails closed before appending over-limit bytes. `wra-57z4` remains open for final all-vmnet buffer/drain closure, so keep this cleanup child blocked until `wra-57z4` and `wra-jenv` are both resolved.

**2026-05-18T09:02:49Z**

Prerequisite update: `wra-57z4` is closed after bounding host-ingress/proxy buffers and per-owner-pass drains. `wra-jenv` is being superseded rather than implemented because the old ByteIo worker premise was deleted by `wra-f762`; current production vmnet already has one owner-driven Tokio fd-readiness/backpressure path for established sessions. This should unblock `wra-ynx7` to delete/quarantine stale sync/dead-code scaffolding instead of adding another established-session worker layer.

**2026-05-18T09:03:54Z**

Started now that `wra-57z4` is closed and `wra-jenv` is superseded. Initial audit target: quarantine sync vmnet frame-pump helpers that are only used by unit tests (`run_qemu_stream_until_eof`, `run_qemu_stream_tick`, `pump_proxy_once`, `pump_host_ingress_once`, and sync guest-frame writer helpers) instead of preserving them as production API. Production remains `serve_vmnet_gateway_async_owner` with Tokio QEMU/service/fd/timer events and async `write_frame_async` output.

**2026-05-18T09:07:41Z**

Iteration 35 first cleanup slice: quarantined/deleted obsolete sync vmnet scaffolding instead of adapting it. `run_qemu_stream_until_eof` and `pump_proxy_once` are now private `#[cfg(test)]` helpers for the remaining unit tests. Deleted unused `VmnetRuntimeTick`, `run_qemu_stream_tick`, sync host-ingress pump helpers, and sync host-ingress event frame writer. Removed stale `#[allow(dead_code)]` from production async guest/proxy/host frame writer helpers. Production still uses `serve_vmnet_gateway_async_owner`, fd readiness snapshots, and async QEMU writes.

Validation: targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features vmnet_runtime::tests:: -- --nocapture` passed; `git diff --check` passed; full `./vm-frontend/validate.sh required` passed at `/tmp/pi-bash-42d47e9f4accf1fb.log`.

**2026-05-18T09:17:09Z**

Iteration 36 cleanup completion: removed/quarantined remaining vmnet transitional dead-code scaffolding. There are no remaining `#[allow(dead_code)]` islands in `vmnet_runtime`, `vmnet_service_io`, `tcp_proxy`, or `vmnet_gateway`. Test-only sync/async harness helpers are marked `#[cfg(test)]`; unused sync tick and host-ingress pump scaffolding was deleted; `run_qemu_stream_until_eof`/`pump_proxy_once` remain private test helpers only. The supervisor launch path now uses the async vmnet boundary directly via `serve_vmnet_gateway_async` instead of spawning a blocking service that creates an inner Tokio runtime; the standalone `vmnet-gateway` CLI keeps the sync wrapper as its external boundary.

Validation: targeted `vmnet_runtime::tests`, `launch::tests`, and `cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features --lib vmnet -- --nocapture` passed. Full required validation passed at `/tmp/pi-bash-6e47da3ee3d2cc40.log`.
