---
id: wra-73tn
status: closed
deps: [wra-zqci]
links: []
created: 2026-05-17T10:19:36Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, vmnet]
---
# Option 1: split vmnet core from async I/O drivers

Prepare vmnet for Tokio by separating pure smoltcp/gateway state from QEMU, host, upstream, pcap, event-log, DNS/connect, and readiness I/O. Current serve_vmnet_gateway owns all of this in one large manual event loop.

## Design

Create boundaries such as VmnetCore for pure smoltcp/gateway frame handling and timers, VmnetIo for QEMU Unix stream framing and pcap capture, HostIngressDriver for accept/read/write, UpstreamDriver for connect/read/write, and EventSink for structured logs. Keep VmnetGateway and smoltcp single-owner and synchronous. Do not put smoltcp behind Arc<Mutex<_>>.

## Acceptance Criteria

Pure vmnet behavior remains testable without sockets, async driver interfaces are clear, QEMU frame codec has bounded async tests, and no duplicated sync/async runtime path is left without a deletion plan.


## Notes

**2026-05-17T10:28:44Z**

Async-boundary refinement: vmnet should be hybrid. Tokio may own QEMU stream readiness, host listener accepts, upstream socket I/O, DNS/connect workers, timers, and cancellation. VmnetGateway/smoltcp state, guest-visible ordering, pcap ordering, and policy decisions tied to frame handling should remain synchronous and single-owner.

**2026-05-17T18:02:20Z**

Continuation-2 iteration 6: started after wra-gq8e reached validation-blocked state. Goal for the first slice is not to rewrite vmnet, but to identify and extract small pure boundaries from vmnet_runtime that prepare for Tokio drivers while keeping VmnetGateway/smoltcp single-owner and synchronous. Avoid Arc<Mutex<_>> around gateway state and keep existing bounded service-worker queues intact.

**2026-05-17T18:05:58Z**

Continuation-2 iteration 6 first slice: introduced a small VmnetCore wrapper around VmnetGateway in vmnet_runtime as the first pure/single-owner core boundary. run_qemu_stream_until_eof and run_qemu_stream_tick now take VmnetCore instead of raw VmnetGateway, while proxy/host helpers still borrow the gateway through a narrow gateway_mut escape hatch until later slices split those drivers. Added vmnet_core_handles_guest_frames_without_runtime_io to prove guest-frame policy behavior remains testable without sockets/poller/QEMU I/O. This keeps smoltcp/gateway state synchronous and single-owner; no Arc<Mutex<_>> introduced. Validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_core -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline stream_runtime_pumps_gateway_and_http_proxy_until_eof -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline proxy_pump_can_deliver_delayed_upstream_response_without_guest_frame -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T18:10:45Z**

Continuation-2 iteration 7: continued VmnetCore extraction without changing ownership/threading. DNS service/deferred-DNS owner-side helpers now take VmnetCore instead of raw VmnetGateway, and serve_vmnet_gateway now owns a VmnetCore for poll delays, timer TCP polling, QEMU guest-frame handling, and DNS worker completion/fail-closed paths. Remaining proxy and host-ingress helper calls still borrow through core.gateway_mut(), keeping the escape hatch explicit for later driver-boundary slices. Full vm-frontend offline validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T18:14:55Z**

Continuation-2 iteration 8: shrank the VmnetCore escape hatch around proxy/TCP-connect owner helpers. pump_proxy_once and pump_proxy_ready now take VmnetCore, and TCP connect service submission/completion/fail-closed helpers now take VmnetCore as well. The production loop no longer passes raw VmnetGateway into proxy pump or TCP connect worker paths; remaining gateway_mut uses are inside the core-facing helpers themselves plus host-ingress open/pump paths, which are the next likely seam. Full vm-frontend offline validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T18:18:18Z**

Continuation-2 iteration 9 reflection/checkpoint: wra-73tn is progressing as intended through small synchronous core-boundary extractions rather than an async rewrite. Accomplished so far: introduced VmnetCore, moved production guest-frame/DNS paths, proxy pump, TCP-connect service owner paths, and now host-ingress open/pump helper signatures behind VmnetCore. Working well: the single-owner smoltcp/VmnetGateway model remains intact; no Arc<Mutex<_>>, no duplicate sync/async runtime path, and full vm-frontend offline validation continues to pass after each slice. Blocking: required/live-capable validation and ticket closure remain blocked by stale appliance artifacts from wra-662v. Approach adjustment: continue only offline-testable mechanical boundary slices until appliance rebuild is confirmed; avoid moving I/O onto Tokio in wra-73tn. Iteration 9 host-ingress slice: pump_host_ingress_once/pump_host_ingress_ready now take VmnetCore, and host ingress session opening is centralized through open_host_ingress_session. The production loop no longer calls host_ingress.open_session directly with raw gateway access. Remaining gateway_mut uses are inside core-facing adapter helpers and one test assertion path. Validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T18:20:01Z**

Continuation-2 iteration 10: tightened/documented VmnetCore adapter boundary. Internal VmnetCore deferred-DNS/completion methods and gateway_mut are no longer pub(crate); gateway_mut is private to vmnet_runtime and documented as the narrow escape hatch for synchronous driver adapters that still call existing proxy/host-ingress APIs. This makes the remaining gateway access intentionally adapter-scoped rather than part of the crate API. Full vm-frontend offline validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check. wra-73tn remains in progress because required/live-capable validation is still blocked by stale appliance artifacts, and further driver-interface cleanup is still useful.

**2026-05-17T18:21:25Z**

Continuation-2 iteration 11: paused feature expansion and did broad offline validation for the current wra-73tn/wra-gq8e/wra-662v state. Assessment: the VmnetCore seam has reached a useful mechanical stopping point for wra-73tn until live-capable validation is available; further cleanup should either be a clearly named driver-adapter extraction or deferred to wra-f762's Tokio actor work. Broad validation passed: cargo test --workspace --offline; cargo fmt --all -- --check. Required validation is still intentionally not retried because docker/out appliance artifacts are stale and need rebuild first.

**2026-05-17T18:29:48Z**

Continuation-2 iteration 12: after the user rebuilt appliance artifacts, ./vm-frontend/validate.sh required passed. This run included the quick live-smoke and live setup-tool scenarios, exercising the production vmnet runtime with the VmnetCore boundary in a live QEMU/KVM guest. Given the current ticket scope, the useful mechanical split is complete: pure gateway/frame behavior is testable via VmnetCore, production owner-side DNS/proxy/TCP-connect/host-ingress paths now go through VmnetCore helper seams, raw gateway access is private/adapter-scoped, and no Arc<Mutex<_>> or duplicate sync/async vmnet path was introduced. Remaining full Tokio actor migration belongs to dependent wra-f762.
