---
id: wra-txoc
status: closed
deps: []
links: []
created: 2026-05-15T09:30:19Z
type: epic
priority: 2
assignee: Henrik Saksela
---
# Readiness-driven vmnet runtime with mio

Replace the current sleep-driven vmnet runtime loop with a readiness-driven design based on mio, while keeping smoltcp owned by one synchronous runtime core so a future Tokio actor rewrite remains possible. The current loop in vm-frontend/src/vmnet_runtime.rs polls QEMU frame IO, smoltcp state, tcp_proxy, host_ingress, event logging, and pcap capture, then sleeps for DEFAULT_VMNET_IDLE_SLEEP when idle. Prior spike wra-0s2s deferred this as a separate runtime design effort and recommended migration order: host_ingress listener/session sockets first, tcp_proxy upstream sockets second, QEMU stream readiness last. The goal is lower idle CPU, better readiness latency, and clearer runtime contracts without changing network policy semantics.

## Design

Keep VmnetGateway and GuestTcpCore synchronous and single-owner. Do not leak mio::Token, mio::Registry, or readiness flags into policy/protocol code. Introduce a narrow runtime event/driver boundary around external IO readiness, smoltcp timer deadlines, and frame writes. Prefer incremental behavior-preserving changes with tests around observable events and gateway/proxy behavior rather than tests tied to raw mio token allocation.

## Acceptance Criteria

The vmnet gateway no longer relies on periodic idle sleeps for host listener/session and upstream socket progress. smoltcp remains serialized behind one runtime owner. Existing vm-frontend offline tests pass. New tests cover the event model and at least host_ingress and tcp_proxy readiness dispatch. Documentation or ticket notes explain the remaining path to a future Tokio actor driver.


## Notes

**2026-05-15T09:45:53Z**

Completed the mio readiness runtime epic. Added a narrow vmnet_poller driver, readiness-aware host_ingress and tcp_proxy bridge entry points, smoltcp poll-delay scheduling, QEMU stream fd readiness handling in serve_vmnet_gateway, docs for the validation/Tokio path, and offline tests for the new event boundary and buffered readiness behavior. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.
