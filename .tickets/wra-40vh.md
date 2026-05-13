---
id: wra-40vh
status: closed
deps: [wra-lxhx]
links: []
created: 2026-05-13T10:21:45Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [network, policy, config, rust]
---
# Define Rust frontend network policy and config model

Define the configuration surface for the Rust userspace network gateway. It must be deny-by-default and cover allowed domains/IPs, blocked ranges, metadata/private ranges, DNS policy, UDP/443 blocking, logging, pcap capture, and CA key/cert paths for HTTPS MITM. This must account for the current wrapper semantics: --no-net, --docker-publish, Docker/payload control readiness, and project-local runtime state under .sandbox/docker-vm/.

## Design

Produce a concrete config schema and runtime defaults before implementing protocol handlers. The schema should separate guest network assignment (guest IP, gateway IP, DNS IP, MTU), egress policy (domain/IP/range allow/deny rules), protocol policy (UDP, QUIC, IPv6, unknown EtherTypes), logging/capture, and TLS MITM CA material. Include how CLI flags map into config and which Python sandbox-wrap flags should be retired or bridged by the Rust frontend.

## Acceptance Criteria

Config schema and default policy are documented. The default is deny-by-default for unsupported protocols and dangerous ranges. The doc states how --no-net and published ports map into the Rust frontend model. Later DNS/TCP/TLS tickets can implement against this schema without redefining policy.


## Notes

**2026-05-13T10:33:59Z**

wra-lxhx confirms the frontend should own the vmnet socket and QEMU should connect with server=off plus reconnect-ms. Policy config should treat unsupported guest traffic as denied at the Rust gateway, and host-to-guest management listeners should be configured separately from guest outbound policy.

**2026-05-13T10:40:46Z**

Policy/config model implemented in vm-frontend/src/network_policy.rs and documented in vm-frontend/network-policy.md. Defaults are deny-by-default for egress, unknown protocols, IPv6, UDP, UDP/443, dangerous private/metadata ranges. --no-net keeps gateway/control-plane semantics but disables guest egress and rejects published ports. --docker-publish maps to frontend-owned HostListener entries, not QEMU hostfwd. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed with 7 tests.
