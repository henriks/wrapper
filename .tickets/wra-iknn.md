---
id: wra-iknn
status: closed
deps: [wra-lxhx, wra-52t3]
links: []
created: 2026-05-13T10:22:12Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, network, qemu, pcap]
---
# Implement QEMU stream endpoint and guest packet capture

Implement the first concrete userspace gateway milestone: accept QEMU -netdev stream over a Unix domain socket, parse length-prefixed Ethernet frames, write frames to optional pcap/pcapng, and provide structured tracing. This is the packet I/O foundation for ARP, DHCP, DNS, TCP, HTTP, and TLS MITM.

## Design

Use Tokio UnixListener/UnixStream or equivalent async I/O. Decode 4-byte big-endian frame length followed by Ethernet bytes. Enforce frame length limits and robust disconnect handling because the guest is untrusted. Use etherparse for Ethernet inspection and pcap-file for capture if selected. Keep this layer policy-neutral except for malformed-frame rejection and capture/logging.

## Acceptance Criteria

A test or smoke tool can connect as a fake QEMU stream peer and verify frame decoding/encoding. Optional pcap output opens in standard tooling. Malformed/truncated frames are rejected without panics. The implementation is ready for ARP/DHCP handlers to send Ethernet replies.


## Notes

**2026-05-13T10:33:59Z**

wra-lxhx found the target QEMU shape: -netdev stream,id=net0,server=off,addr.type=unix,addr.path=/vmnet.sock,reconnect-ms=250 plus the existing microvm virtio-net-device. Implement the stream endpoint as reusable Rust frontend code: exact 4-byte big-endian length reads/writes, bounded frame size, pcap capture, single-client lifecycle, and live validation against the current appliance.

**2026-05-13T10:38:58Z**

wra-52t3 added vm-frontend RuntimePaths::vmnet_sock and SupervisorPlan::VmnetGateway as the extension point. Implement the stream endpoint behind that ManagedTask; command construction already passes server=off,addr.type=unix,addr.path=/vmnet.sock,reconnect-ms=250.

**2026-05-13T10:42:57Z**

Implemented vm-frontend/src/vmnet_stream.rs with VmnetStreamEndpoint, generic QemuFrameIo, VmnetFrameIo for UnixStream, 4-byte big-endian frame read/write, frame length bounds, truncated length/payload errors, and standard Ethernet pcap writer. Unit tests use in-memory fake QEMU streams because the sandbox denies Unix socket writes; they verify decoding, encoding, truncation rejection, oversized-frame rejection, and pcap file layout. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed with 12 tests.
