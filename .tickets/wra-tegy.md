---
id: wra-tegy
status: closed
deps: [wra-ug25]
links: []
created: 2026-05-15T09:30:47Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-txoc
---
# Add narrow mio vmnet poll driver

Add mio as the readiness driver for vmnet runtime without moving smoltcp into async tasks. Relevant code: vm-frontend/Cargo.toml, vm-frontend/src/vmnet_runtime.rs, and any new runtime-driver module. This should introduce Poll/Events/Token ownership in one narrow layer and map readiness back into the event boundary from wra-ug25. The current periodic sleep path may remain as a fallback during this ticket, but new code should make it possible for later tickets to register host listener/session and upstream sockets.

## Design

Keep token allocation and interest updates centralized. Prefer a small RuntimePoller type that owns mio::Poll, Events, token generation, and readiness dispatch. Do not expose mio types through VmnetGateway, GuestTcpCore, HostIngressBridge public policy methods, or TcpProxyBridge policy methods. Use nonblocking std net sockets via mio net wrappers or careful conversion only at the driver edge.

## Acceptance Criteria

vm-frontend has a narrow mio dependency and a tested runtime poller/event dispatch skeleton. Existing vmnet behavior remains intact. New tests cover token registration, readiness dispatch, timeout/deadline handling, and deregistration behavior with fake or local sockets. cargo test --manifest-path vm-frontend/Cargo.toml --offline passes.


## Notes

**2026-05-15T09:44:16Z**

Added direct mio dependency with net/os-ext/os-poll features and RuntimePoller as the narrow driver. RuntimePoller owns Poll, Events, Token allocation, SourceFd registration/reregistration/deregistration, and maps readiness back to VmnetEventSource. Tests cover registration, reregistration, deregistration, and timeout polling without requiring sandbox-blocked loopback networking.

**2026-05-15T09:46:38Z**

Follow-up fix after validation: RuntimePoller::deregister now tolerates EBADF/ENOENT because bridge code may drop a closed std socket before the runtime syncs poller state. The token/source maps are still cleaned before returning.
