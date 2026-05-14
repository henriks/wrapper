---
id: wra-f16x
status: closed
deps: []
links: []
created: 2026-05-14T20:12:13Z
type: epic
priority: 1
assignee: Henrik Saksela
tags: [tests, sandbox, fuzzing]
---
# Expand sandbox bug-finding test suite

Expand the Rust test suite with the explicit goal of finding critical sandbox correctness bugs. The target surfaces are the product core: composed filesystem isolation and vmnet/network isolation. Prioritize randomized, adversarial, and model-based tests over coverage-only examples. Relevant crates/files: composed-fs/src/lib.rs for virtiofs namespace/path/handle behavior; vm-frontend/src/vmnet_stream.rs for QEMU frame IO; vm-frontend/src/vmnet_gateway.rs for Ethernet/IP/UDP/TCP frame handling; vm-frontend/src/guest_tcp.rs for TCP SYN parsing and smoltcp integration; vm-frontend/src/dns_proxy.rs and tcp_gateway.rs for policy decisions.

## Acceptance Criteria

- Child tickets cover shared fuzz/test harness setup, network byte fuzzing, gateway policy fuzzing, composed-fs model expansion, fs race/path escape tests, policy differential tests, and end-to-end hostile guest smoke tests.
- Dependencies reflect implementation order.
- Each child ticket has enough context for implementation without relying on prior conversation.


## Notes

**2026-05-14T20:32:59Z**

Completed all child tickets for expanding the sandbox bug-finding test suite. Added frontend property/fuzz plumbing, vmnet stream frame proptests, vmnet gateway generated frame tests, TCP/DNS policy differential properties, composed-fs nested model proptests, composed-fs host mutation/path escape regressions, and a self-test --hostile smoke profile with ignored manual runner. Final verification performed with cargo test --manifest-path vm-frontend/Cargo.toml --offline and cargo test --manifest-path composed-fs/Cargo.toml --offline; tk dep cycle reports no cycles.
