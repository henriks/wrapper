---
id: wra-b6iu
status: closed
deps: [wra-z9sh]
links: []
created: 2026-05-13T20:56:07Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, network, qemu, control]
---
# Propagate guest TCP close to host ingress clients

Host ingress now supports Docker, payload, and published-port loopback listeners over the Rust vmnet gateway, but live publish smoke exposed a remaining TCP lifecycle gap: when a guest service sends a response and closes, the frontend can deliver the response bytes to the host connection while the host client still does not reliably observe EOF. Clients that parse Content-Length work, but raw TCP/HTTP clients waiting for close can time out. Relevant code: vm-frontend/src/host_ingress.rs, vm-frontend/src/guest_tcp.rs, vm-frontend/src/vmnet_gateway.rs, vm-frontend/src/vmnet_runtime.rs.

## Design

Track guest-side TCP state transitions for host-ingress sessions and propagate guest FIN/close to the host connection. Avoid adding QEMU usernet/hostfwd or a second forwarding stack. Prefer using smoltcp socket state/recv semantics and shutting down/removing the corresponding host connection when the guest side closes, while still flushing any final guest payload bytes first.

## Acceptance Criteria

A live Rust frontend smoke with --publish HOST:GUEST can run a one-shot guest TCP/HTTP server that sends a response and closes; the host client receives the full response body and EOF without timing out. Regression coverage exercises guest-close handling in HostIngressBridge. Document the observed outcome in the ticket before closing.


## Notes

**2026-05-13T20:56:38Z**

Started immediately after wra-z9sh live smoke. Repro signal: one-shot guest TCP server over --publish delivered response bytes in vmnet-events.log, but the host probe waited for EOF and timed out; repeated later connects saw refusal because the one-shot server had already exited. Fix should be in host-ingress session lifecycle, not by adding hostfwd or another forwarding path.

**2026-05-13T20:59:42Z**

Implemented guest close propagation in HostIngressBridge. The bridge now processes guest closed states after flushing final guest bytes, emits host_ingress_guest_closed, drops the host connection so clients observe EOF, and removes the bridge session. Regression coverage extends host_ingress::bridges_host_payload_into_guest_session_and_guest_response_back with a guest FIN path and keeps the host EOF cleanup test. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed; live Rust launch with --publish 18080:18080 ran a one-shot guest server and host received 'HTTP/1.1 200 OK publish-ok' plus payload-exit 0. vmnet-events.log recorded host_ingress_guest_closed state=CloseWait.
