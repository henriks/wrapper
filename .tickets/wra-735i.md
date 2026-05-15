---
id: wra-735i
status: open
deps: [wra-reoq]
links: []
created: 2026-05-15T06:51:20Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-2cfd
tags: [network, fuzzing, proptest]
---
# Add structured vmnet packet and session fuzzing

Extend vmnet fuzz/property coverage beyond arbitrary byte frames and one-shot SYN policy checks. Relevant code: vm-frontend/src/vmnet_gateway.rs, vm-frontend/src/vmnet_stream.rs, vm-frontend/src/tcp_gateway.rs, vm-frontend/src/dns_proxy.rs, vm-frontend/src/test_support.rs. Add structured generators for Ethernet/ARP/IPv4/UDP/TCP/DNS packets with deliberate malformed variants: bad lengths, checksums, unknown ethertypes, unsupported protocols, fragmented IPv4, DNS compression/name edge cases, QUIC/UDP denial, TCP retransmits, out-of-order segments, FIN/RST races, repeated SYNs, and session teardown/backpressure scenarios. Assert fail-closed behavior, bounded output, no unexpected session growth, and policy decisions matching evaluate_tcp_destination/domain policy.

## Acceptance Criteria

Network property tests include structured packet/session sequences, not only arbitrary bytes. Generated cases reach DNS, UDP, TCP accept/deny, unsupported protocol, and malformed packet branches. Invariants include bounded guest output, fail-closed policy behavior, no leaked active sessions after denied/malformed traffic, and deterministic failure traces or seeds.

