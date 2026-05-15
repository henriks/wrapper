---
id: wra-ssve
status: open
deps: [wra-tegy]
links: []
created: 2026-05-15T09:30:59Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-txoc
---
# Move host ingress onto readiness events

Replace host_ingress periodic accept/read/write probing with readiness-driven dispatch through the vmnet runtime poller. Relevant code: vm-frontend/src/host_ingress.rs HostIngressListenerSet::bind/accept_pending and HostIngressBridge::process_gateway, plus vm-frontend/src/vmnet_runtime.rs pump_host_ingress_once. Current behavior loops over nonblocking listeners until WouldBlock and probes each active connection every runtime tick. The new behavior should register listeners and active host connections, accept only when listener readiness fires, read/write host sessions when readiness indicates progress, and keep all smoltcp mutation serialized through VmnetGateway.

## Design

HostIngressBridge may expose readiness-specific operations such as accept_ready, host_read_ready, host_write_ready, or drain_guest_payload, but it should not own the global mio Poll. Token mapping belongs in the runtime driver. Preserve event emission names and semantics where possible: Opened, OpenFailed, HostPayload, GuestPayload, HostClosed, GuestClosed, and read/write failure events.

## Acceptance Criteria

Host ingress listener accepts and host session read/write progress are driven by readiness events rather than unconditional per-tick scanning. Existing host_ingress tests pass, with added coverage for readiness-triggered accept, readable host payload, writable guest payload flush, guest close propagation, and deregistration on close/failure. cargo test --manifest-path vm-frontend/Cargo.toml --offline passes.

