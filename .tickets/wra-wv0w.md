---
id: wra-wv0w
status: closed
deps: []
links: [wra-57z4]
created: 2026-05-18T05:37:06Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-9m5h
tags: [cleanup, network-policy, vmnet]
---
# Parse network policy ranges once into typed policy structures

Network policy handling still passes string ranges across layers that later reparse them in gateway/proxy code. That keeps validation, failure handling, and policy semantics scattered. Parse IP/CIDR ranges once at the config or CLI boundary into typed policy structures, pass those through runtime code, and delete downstream string parsing adapters.

## Design

Audit network_policy configuration loading, launch request construction, tcp_gateway/vmnet consumers, and tests. Preserve documented .sandbox/config.json semantics and update vm-frontend/config-json.md if policy field behavior changes. Add fuzz or property-style coverage if arbitrary policy input parsing is changed.

## Acceptance Criteria

Runtime network code receives typed policy data, not raw range strings; duplicate parsing and string adapter helpers are removed; invalid policy input fails at the boundary with a clear error; docs/tests/fuzz coverage are updated as required; required validation is recorded before close.


## Notes

**2026-05-18T06:46:14Z**

Started implementation. Direction: replace runtime string IP/CIDR range reparsing with typed IPv4 range policy data. Network config and CLI still accept the documented allowed_ips/--allow-ip strings at the boundary, then policy_from_args/vmnet CLI parse to typed ranges before runtime TCP/vmnet code sees the policy.

**2026-05-18T06:48:30Z**

Implemented typed IPv4 policy ranges. EgressPolicy now stores allow_ip_ranges and deny_ip_ranges as Ipv4Range values parsed once by policy_from_args/vmnet CLI; tcp_gateway runtime checks no longer parse strings per decision. Invalid --allow-ip/allowed_ips values now fail at the launch/vmnet boundary with an invalid --allow-ip diagnostic. Updated config-json.md, unit/property tests, and added fuzz target ipv4_policy_range. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features; cargo check --manifest-path vm-frontend/fuzz/Cargo.toml --offline --bins; ./vm-frontend/validate.sh required (success, including live-smoke and live-setup-tools; full log /tmp/pi-bash-6e4a6f066f882aae.log).
