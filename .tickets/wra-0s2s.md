---
id: wra-0s2s
status: closed
deps: []
links: []
created: 2026-05-15T08:56:21Z
type: task
priority: 3
assignee: Henrik Saksela
parent: wra-sne0
---
# Evaluate readiness-driven vmnet runtime with mio

Spike whether mio should replace sleep-driven/nonblocking polling across vmnet runtime, tcp_proxy, and host_ingress. Relevant code: vm-frontend/src/vmnet_runtime.rs pump loop, vm-frontend/src/tcp_proxy.rs session/upstream write handling, vm-frontend/src/host_ingress.rs listener accept/session bridge logic. Current code manually handles WouldBlock, partial writes, listener accept loops, session maps, and periodic sleeps. Candidate crate: mio; tokio is a larger architectural option but likely high risk with smoltcp.

## Acceptance Criteria

A checked-in note or ticket note documents whether mio is worth adopting, which loops it would replace, what stays custom, and the migration order. No runtime rewrite is required in this spike. If adopted, follow-up implementation tickets are created with dependencies.


## Notes

**2026-05-15T09:20:18Z**

Spike outcome: do not adopt mio in this crate-replacement epic. The current vmnet runtime loop coordinates smoltcp polling, QEMU frame IO, host ingress, tcp_proxy backpressure, and event logging with tight domain-specific state. A readiness API would be worthwhile only as a separate runtime design epic, starting with listener/session sockets in host_ingress, then tcp_proxy upstream sockets, and only then QEMU stream readiness. Tokio remains higher risk because it would force broader async boundaries through smoltcp and payload/control code. No runtime rewrite was made here.
