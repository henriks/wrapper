---
id: wra-7ixt
status: closed
deps: [wra-ay63]
links: []
created: 2026-05-14T20:12:43Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-f16x
tags: [tests, fuzzing, network, vmnet]
---
# Fuzz vmnet gateway guest frame handling

Add randomized/adversarial tests around VmnetGateway::handle_guest_frame in vm-frontend/src/vmnet_gateway.rs, plus the lower-level parsers it uses in guest_tcp.rs and l2_gateway.rs. This is the core guest network isolation boundary: arbitrary Ethernet frames from the guest should not panic, allocate unboundedly, create unintended TCP sessions, or slip past policy. Existing tests cover ARP, DHCP, DNS, UDP denial, TCP accepted/denied, and unsupported protocols with hand-built examples.

## Design

Use generated arbitrary Ethernet frames and structured frame builders. Structured cases should vary ethertype, IPv4 version/IHL/total_len/protocol, UDP length/ports, TCP flags/options, checksum validity, frame truncation, and MTU-adjacent sizes. Invariants should be policy-oriented, not just parser-oriented: malformed frames produce Ignored/Unsupported/denied outcomes; non-DNS UDP is denied or unsupported; UDP/443 is denied before any forwarding; denied TCP SYNs do not create active sessions; accepted sessions happen only when policy allows the destination.

## Acceptance Criteria

- A generated-frame test runs through handle_guest_frame without panic for malformed/truncated/random frames.
- Policy invariants are asserted for UDP/443, non-DNS UDP, unknown ethertypes, IPv6, malformed IPv4, and denied TCP SYNs.
- Random denied TCP inputs cannot create active_tcp_sessions.
- Any generated guest_frames are bounded and parseable enough not to violate the configured MTU expectations.


## Notes

**2026-05-14T20:21:38Z**

Added VmnetGateway property tests for arbitrary guest frames, structured UDP frames, unknown ethertypes/IPv6, denied TCP SYNs, and malformed IPv4 payloads. Invariants assert no panic, bounded guest frame outputs, fail-closed UDP/unknown protocol behavior, and no active TCP sessions for denied/non-accepted inputs. Added ignored stress_seeded_generated_guest_frames with documented cargo test command. Verification: cargo test --manifest-path vm-frontend/Cargo.toml vmnet_gateway::tests --offline. One implementation-only issue found: prop_assert! stringifies patterns with { .. } as format strings, patched to explicit TestCaseError branches.
