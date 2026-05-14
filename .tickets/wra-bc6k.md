---
id: wra-bc6k
status: closed
deps: [wra-y325]
links: []
created: 2026-05-14T18:41:43Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-tad5
tags: [validation, testing, network]
---
# Add DNS host-ingress and published-port integration coverage

Add in-process and, where necessary, ignored local integration tests for DNS proxy behavior, host-to-guest ingress, Docker listener forwarding, payload-control forwarding, and arbitrary published ports.\n\nDNS cases: allowed/blocked queries, wildcard domains, upstream SERVFAIL/unavailable, malformed queries, CNAME/A/AAAA handling where currently supported, repeated queries, logging detail, and policy interaction. Host ingress cases: connect/open failures, payload delivery, guest responses, guest close, host close, backpressure, multiple listeners, and --no-net interaction with control listeners.\n\nRelevant code: vm-frontend/src/dns_proxy.rs, host_ingress.rs, vmnet_gateway.rs, vmnet_runtime.rs, main.rs policy parsing.

## Acceptance Criteria

Tests cover DNS and host-ingress behavior without relying on external network access. Published-port and Docker/payload listener semantics have regression coverage. Notes document which behavior is in-process only and which requires live QEMU.


## Notes

**2026-05-14T19:49:29Z**

Added deterministic DNS proxy tests for blocked domains not calling upstream, repeated allowed queries being forwarded without cache/coalescing, multi-question FormErr handling without upstream calls, and preserving upstream CNAME+AAAA answers. Added host-ingress bridge tests for multiple simultaneous host sessions/local-port allocation, host read failure closure, and host write failure closure. Added an ignored loopback listener-set integration test proving DockerApi, PayloadControl, and PublishedTcp listener purposes/guest ports are preserved through accept_pending; ignored because this command sandbox blocks loopback bind/connect. Offline vm-frontend tests pass.
