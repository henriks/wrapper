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

**2026-05-15T10:06:33Z**

Live validation exposed a host-ingress readiness gap after the mio rewrite: the readiness path read at most one host chunk per readable notification. With payload control, that could consume only the 5-byte frame header and leave the JSON body buffered, causing repeated ping success but wait_for_payload_ready timeout. Fixed HostIngressBridge::process_gateway_with_readiness to drain host reads until WouldBlock and added readiness_drains_multiple_host_reads_until_would_block regression coverage.

**2026-05-15T10:30:16Z**

Full live validation after the mio rewrite exposed additional contracts beyond the first host-read drain bug: (1) host payload data can arrive before a newly established host-ingress session is registered with mio, so vmnet_runtime now polls all host-ingress sessions after QEMU-side TCP progress; (2) guest responses should be written opportunistically when available and only buffered on WouldBlock; (3) rebuilt appliance artifacts were required because docker/guest-payload-server.py was newer than docker/out; (4) guest-init must bring lo up so guest DOCKER_HOST=tcp://127.0.0.1:1075 stays inside the guest instead of hitting vmnet loopback-deny policy; (5) Docker resolver sends EDNS additional OPT records, so dns_proxy now allows additional records while still rejecting answer/authority records in queries. Live validation passes after these fixes.
