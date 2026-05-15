---
id: wra-ug25
status: closed
deps: []
links: []
created: 2026-05-15T09:30:35Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-txoc
---
# Define vmnet runtime event boundary

Introduce an internal event model for vmnet runtime before wiring in mio. Relevant code: vm-frontend/src/vmnet_runtime.rs serve_vmnet_gateway/run_qemu_stream_tick/pump_proxy_once/pump_host_ingress_once, vm-frontend/src/vmnet_gateway.rs VmnetGateway, vm-frontend/src/host_ingress.rs HostIngressBridge/HostIngressListenerSet, and vm-frontend/src/tcp_proxy.rs TcpProxyBridge. The event model should describe external IO readiness, smoltcp timer deadlines, QEMU frame availability, host listener accepts, host session read/write readiness, upstream proxy read/write readiness, shutdown/EOF, event-log writes, and pcap capture side effects. This ticket should keep smoltcp owned by one synchronous runtime owner and avoid adding mio-specific types to gateway/proxy policy code.

## Design

Create small domain types or traits in/near vmnet_runtime for readiness events and runtime actions. Keep the current blocking/nonblocking behavior working while establishing names and seams for later tickets. Tests should exercise event-to-pump ordering with fake inputs where practical, but this ticket does not need to replace the loop yet.

## Acceptance Criteria

There is a checked-in event/boundary abstraction or documented in-code structure that later mio tickets can use. VmnetGateway/GuestTcpCore remain synchronous and single-owner. No mio::Token or mio::Registry appears in vmnet_gateway, guest_tcp, network_policy, or tcp_gateway. cargo test --manifest-path vm-frontend/Cargo.toml --offline passes.


## Notes

**2026-05-15T09:44:02Z**

Implemented vmnet runtime event boundary around VmnetEventSource/VmnetReadyEvent/VmnetInterest in vm-frontend/src/vmnet_poller.rs. VmnetGateway and GuestTcpCore remain synchronous and single-owner; no mio types were added to vmnet_gateway, guest_tcp, network_policy, or tcp_gateway. Added smoltcp poll-delay exposure for runtime timer scheduling.
