---
id: wra-cz7d
status: closed
deps: [wra-iknn, wra-40vh]
links: []
created: 2026-05-13T10:22:22Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, network, arp, dhcp]
---
# Implement virtual L2 IPv4 gateway with ARP and DHCP

Make the Rust userspace gateway usable as the guest network gateway by implementing Ethernet generation, ARP responses for the gateway IP, and a DHCPv4 server that gives the microvm guest a fixed lease, gateway, DNS, MTU, and any required options. This replaces the current QEMU user-mode network address assignment in the Rust frontend path.

## Design

Use the config model for guest MAC/IP, gateway IP, DNS IP, lease duration, and MTU. Respond only to the configured guest/network and deny or ignore unexpected clients. Use dhcproto where useful, but keep frame handling explicit enough to audit. Document any guest image assumptions, such as DHCP client behavior and virtio-net interface naming.

## Acceptance Criteria

The guest can obtain IPv4 configuration through the Rust gateway when booted with QEMU stream networking. ARP for the gateway IP works. Unsupported EtherTypes and non-configured clients are ignored or denied with logs. Behavior is covered by unit tests using captured/fake Ethernet frames and by a VM smoke note when available.


## Notes

**2026-05-13T10:40:46Z**

wra-40vh defines VmnetPolicy.assignment and protocol defaults. ARP/DHCP should use GuestNetwork values from policy and keep IPv4-only behavior; IPv6 and unknown EtherTypes remain denied/logged.

**2026-05-13T10:42:57Z**

wra-iknn added QemuFrameIo::write_frame/read_frame and PcapWriter. ARP/DHCP can now be implemented as handlers over decoded Ethernet frames and can send replies through the same frame writer.

**2026-05-13T10:44:54Z**

Implemented initial L2 IPv4 gateway in vm-frontend/src/l2_gateway.rs. L2Gateway consumes GuestNetwork values, replies to ARP requests for the gateway IP, parses IPv4/UDP DHCP client traffic, and emits DHCP offer/ack frames for the fixed guest lease with gateway, DNS, subnet mask, lease time, and MTU options. Unit tests verify ARP reply shape and DHCP offer contents. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed with 14 tests.
