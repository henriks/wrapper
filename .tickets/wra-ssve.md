---
id: wra-ssve
status: closed
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


## Notes

**2026-05-15T09:44:36Z**

Moved host ingress toward readiness dispatch. HostIngressListenerSet now exposes listener fds and accept_ready(index). HostIngressBridge has HostIngressReadiness, per-session interests, raw fd access for TcpStream sessions, and pending host-write buffering so guest payload is not dropped when the host socket is not writable. vmnet_runtime registers/deregisters host sessions through RuntimePoller. Added readiness_buffers_guest_payload_until_host_socket_is_writable regression coverage.
