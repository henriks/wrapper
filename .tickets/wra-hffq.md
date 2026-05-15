---
id: wra-hffq
status: closed
deps: [wra-o9y4, wra-kh0g]
links: []
created: 2026-05-15T10:38:50Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-neci
tags: [validation, dns, network]
---
# Exercise DNS behavior with deterministic, fuzz, and live resolver tests

The recent live failure exposed that DNS behavior was not covered well enough. Add coverage around vm-frontend/src/dns_proxy.rs and its integration with the runtime/network path. Include deterministic unit/table tests for EDNS OPT handling, TCP fallback-sized responses, NXDOMAIN/SERVFAIL/REFUSED behavior, CNAME chains, IPv4/IPv6 answers, malformed packets, truncated packets, and allow/deny host filtering. Add fuzz or property tests for arbitrary DNS byte input so parser/proxy code never panics or spins. Add live validation cases that prove allowed names resolve through the guest and denied names fail with the expected observable error.

## Design

Prefer a local fixture resolver for deterministic offline tests. Live tests should be named scenarios under the validation matrix rather than opaque smoke behavior. Keep assertions at the contract level: answer class/rcode, host policy decision, no panic, no session leak, and useful diagnostic output.

## Acceptance Criteria

Offline tests cover malformed and valid DNS packets, EDNS, policy allow/deny, and representative rcodes. Fuzz/property coverage exists for arbitrary DNS input. Live validation includes at least one allowed resolver path and one denied resolver path from inside the guest. Failure messages identify the queried host and policy/rcode involved.


## Notes

**2026-05-15T12:01:06Z**

Implemented/validated DNS scenario coverage. Existing offline DNS coverage already includes malformed/truncated handling, EDNS additional records, allow/deny policy, CNAME+AAAA, SERVFAIL, REFUSED, gateway destination filtering, and fuzz target vm-frontend/fuzz/fuzz_targets/dns_proxy_payload.rs for arbitrary DNS payloads. Added explicit NXDOMAIN preservation test and self-test --dns-check. Added validate.sh live-dns with allowed resolver path (--dns-check) and denied resolver path (--hostile --no-net), with guest diagnostics dns-allow-ok/dns-allow-failed and dns-deny-ok/dns-deny-unexpected identifying example.com. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline dns_proxy::tests:: -- --nocapture passed (14 passed/1 ignored); cargo test --manifest-path vm-frontend/Cargo.toml --offline self_test -- --nocapture passed; ./vm-frontend/validate.sh live-dns passed both allowed and denied KVM scenarios; ./vm-frontend/validate.sh required passed end-to-end.
