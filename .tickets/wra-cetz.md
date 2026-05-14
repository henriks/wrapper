---
id: wra-cetz
status: closed
deps: [wra-7ixt]
links: []
created: 2026-05-14T20:13:09Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-f16x
tags: [tests, fuzzing, network, policy]
---
# Add network policy differential property tests

Add property/differential tests for network policy behavior across vm-frontend/src/network_policy.rs, dns_proxy.rs, tcp_gateway.rs, guest_tcp.rs, and vmnet_gateway.rs. The sandbox promise depends on all policy entry points agreeing about what is allowed or denied. Existing tests check representative defaults, allowed domains/IPs, deny ranges, no-net, DNS decisions, and TCP decisions, but do not broadly generate policy/destination combinations.

## Design

Generate VmnetPolicy variants and destinations/domains. Compare pure decisions from evaluate_tcp_destination/domain_allowed/dns_allowed_to_destination with frame-level outcomes from VmnetGateway where structured frame builders can express the same destination. Key invariants: deny_ranges always win even when allow_ips/default public egress also match; metadata/link-local/private/loopback/multicast/reserved ranges remain denied; default deny denies unless explicit allow matches; wildcard domain matching is case-insensitive and dot-normalized; HTTPS interception denies if MITM material is incomplete.

## Acceptance Criteria

- Generated tests cover combinations of default deny/public egress, allow_ips, deny_ranges, allow_domains, wildcard domains, and MITM config.
- Pure policy decisions and gateway frame outcomes agree for generated TCP SYN and DNS/UDP cases where both are applicable.
- Deny ranges are asserted to take precedence over all allow mechanisms.
- Domain normalization/case/trailing-dot behavior is covered.


## Notes

**2026-05-14T20:23:59Z**

Added network policy differential properties. tcp_gateway now generates policy/destination combinations and compares evaluate_tcp_destination against VmnetGateway SYN outcomes, asserts deny_ranges override public egress, allow_ips, allow_domains, and verifies HTTPS interception requires complete MITM config. vmnet_gateway now generates DNS domain/policy combinations and compares gateway DnsQuery decisions against the pure domain_allowed result, covering default public egress, exact allow_domains, wildcard allow_domains, uppercase queries, and trailing dots. Verification: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed with 128 lib tests + 21 main tests run, 5 ignored.
