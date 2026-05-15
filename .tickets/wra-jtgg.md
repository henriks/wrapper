---
id: wra-jtgg
status: closed
deps: []
links: []
created: 2026-05-15T08:56:13Z
type: feature
priority: 2
assignee: Henrik Saksela
parent: wra-sne0
---
# Replace offset-based packet parsing with packet APIs

Reduce raw Ethernet/IPv4/UDP/ARP byte parsing and construction in vm-frontend. Primary targets: vm-frontend/src/vmnet_gateway.rs parse_dns_query_frame, parse_udp_frame, and DNS response packet construction; vm-frontend/src/test_support.rs UDP/IPv4/Ethernet/ARP packet fixtures. Candidate approach: first lean on existing smoltcp::wire packet/repr APIs; evaluate etherparse if smoltcp remains awkward. Keep manual mutation helpers only for tests that intentionally create malformed frames.

## Acceptance Criteria

Raw magic offsets in the primary UDP/DNS packet path are substantially reduced or isolated behind packet helper APIs. Checksums/lengths are produced by packet APIs where practical. Malformed-frame tests remain possible. cargo test --manifest-path vm-frontend/Cargo.toml --offline passes, including vmnet gateway property tests in the normal suite.


## Notes

**2026-05-15T09:19:14Z**

Reworked vmnet_gateway UDP/DNS parsing to use smoltcp::wire EthernetFrame, Ipv4Packet, and UdpPacket rather than raw offsets. Malformed UDP paths still use packet check_len semantics and existing malformed/property tests pass. DNS response construction remains manual because the current code intentionally emits UDP checksum zero and the narrow writer is still straightforward. Validation: vm-frontend offline tests and fmt check pass.
