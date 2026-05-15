---
id: wra-pido
status: open
deps: [wra-ssve]
links: []
created: 2026-05-15T09:31:10Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-txoc
---
# Move tcp proxy upstream sockets onto readiness events

Replace tcp_proxy upstream socket probing with readiness-driven dispatch. Relevant code: vm-frontend/src/tcp_proxy.rs TcpProxyBridge::process_gateway, UpstreamSession pending_upstream_bytes/pending_upstream_plaintext/pending_guest_bytes, TLS MITM handling, and vm-frontend/src/vmnet_runtime.rs pump_proxy_once. Current behavior walks active smoltcp sessions and opportunistically reads/writes upstream connections every runtime tick. The new behavior should register upstream connections, update interest based on pending writes/TLS handshake needs, and process read/write readiness without losing partial-write or backpressure semantics.

## Design

Keep TcpProxyBridge responsible for protocol state and buffering, but keep mio registration/token ownership in the runtime driver. Be especially careful with HTTPS MITM: guest TLS writes, upstream TLS writes, pending plaintext, and upstream TLS readiness all need explicit interest updates. Preserve existing TcpProxyEvent names and ordering where observable tests rely on them.

## Acceptance Criteria

Upstream TCP proxy read/write progress is driven by readiness events rather than unconditional per-tick probing. Existing tcp_proxy tests pass, including large upstream responses, partial guest sends, TLS handshake payload, and pending write behavior. Add tests for readiness-driven upstream read, readiness-driven pending write flush, connection close/failure deregistration, and TLS interest updates. cargo test --manifest-path vm-frontend/Cargo.toml --offline passes.

