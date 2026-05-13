---
id: wra-zqua
status: closed
deps: [wra-cz7d, wra-40vh]
links: []
created: 2026-05-13T10:22:32Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, network, dns, policy]
---
# Implement DNS proxy with deny-by-default policy hooks

Implement DNS handling on the gateway IP so the untrusted guest cannot bypass policy through arbitrary resolvers. The gateway should proxy allowed DNS queries to configured upstream resolvers, log queries/results, and expose policy hooks for domain/IP decisions used by later TCP and MITM handling.

## Design

Use hickory-proto or equivalent for DNS message parsing/generation. Handle UDP/53 and decide whether TCP/53 is in scope for v1. Apply the config policy before proxying and before returning answers. Cache only if it simplifies policy and is documented. DNS decisions should feed later outbound TCP policy, but this ticket should not implement TCP forwarding.

## Acceptance Criteria

Guest DNS queries to the gateway IP resolve allowed names through a host-side upstream resolver. Blocked domains return a deterministic denial response and are logged. Attempts to use non-gateway DNS are denied by the network policy. Unit tests cover allowed, blocked, malformed, and upstream-failure cases.


## Notes

**2026-05-13T10:40:46Z**

wra-40vh defines DNS policy as UDP allowed only to the gateway DNS proxy by default, with domain allow rules living in EgressPolicy.allow_domains. DNS implementation should log and enforce against VmnetPolicy rather than defining separate policy knobs.

**2026-05-13T10:44:54Z**

wra-cz7d now provides the L2/IP foundation and DHCP assigns DNS to the gateway DNS IP. DNS proxy implementation should handle guest UDP/53 frames after L2Gateway has established the fixed lease.

**2026-05-13T10:59:07Z**

Implemented DNS proxy policy layer in vm-frontend/src/dns_proxy.rs using hickory-proto v0.26.1 for DNS Message parsing/serialization. Added DnsProxy, DnsUpstream trait, UdpDnsUpstream for host-side UDP resolver forwarding, deterministic REFUSED for blocked domains, SERVFAIL for upstream failures, logging decisions, wildcard/domain allow matching against VmnetPolicy, and dns_allowed_to_destination() to deny non-gateway DNS. Unit tests cover allowed, blocked, malformed, upstream-failure, wildcard allow, and non-gateway destination cases. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed with 20 tests; composed-fs tests still pass.
