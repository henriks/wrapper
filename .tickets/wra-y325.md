---
id: wra-y325
status: closed
deps: [wra-gx4r]
links: []
created: 2026-05-14T18:41:21Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-tad5
tags: [validation, testing, network]
---
# Expand network packet and policy unit coverage

Add exhaustive fast offline unit tests for packet parsing/generation and policy decisions. Cover QEMU stream framing; Ethernet, ARP, DHCP, IPv4, TCP, UDP, and DNS parsing/generation; malformed/truncated/oversized frames; checksum behavior where applicable; and unsupported protocol handling.\n\nPolicy coverage must include default deny, public egress, --no-net, allow-domain, allow-IP, wildcard domain handling, private/metadata deny ranges, IPv6 deny, UDP/443 deny, unknown IPv4 protocols, DNS only to gateway IP/port, and event-log decision text.\n\nRelevant code: vm-frontend/src/vmnet_stream.rs, l2_gateway.rs, dns_proxy.rs, network_policy.rs, vmnet_gateway.rs, tcp_gateway.rs.

## Acceptance Criteria

Fast offline tests cover the full packet and policy matrix from the validation document. Each denied/allowed case asserts both behavior and diagnostic/log detail where the code emits it. Notes summarize any protocol cases intentionally unsupported.


## Notes

**2026-05-14T18:52:05Z**

Reuse vm-frontend/src/test_support.rs for packet/policy tests instead of creating duplicate fixtures. Relevant helpers: qemu_stream_bytes, memory_qemu_frame_io, default_guest_network, smol_time_ms, FrontendFixture for runtime paths, TestCa for TLS-policy setup, and StaticTcpServer for deterministic local upstream behavior.

**2026-05-14T18:56:17Z**

Expanded fast offline network coverage and fixed a behavior gap found by the new tests. Added tests for QEMU zero/truncated frame handling, ARP non-gateway/malformed ignores, malformed/non-DHCP UDP, no-net policy reason, default deny ranges, wildcard/domain/public TCP policy, denied-range precedence, unsupported UDP, UDP/443, unsupported IPv6, unknown ethertypes, unknown IPv4 protocols, malformed UDP, and unsupported-protocol event log formatting. Production change: VmnetGateway now classifies unsupported protocols with GuestFrameOutcome::UnsupportedProtocol and vmnet runtime logs unsupported_protocol rather than letting those frames fall through into the TCP pump as TcpProgress. Verification: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed with 93 lib tests passed, 1 ignored, and 18 bin tests passed.
