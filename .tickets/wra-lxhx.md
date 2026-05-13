---
id: wra-lxhx
status: closed
deps: []
links: []
created: 2026-05-13T10:21:36Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [spike, network, qemu, rust]
---
# Spike QEMU stream userspace vmnet gateway feasibility

Validate the precise QEMU stream-netdev contract for the Rust frontend network pivot. Current system uses QEMU user-mode networking plus hostfwd for guest Docker/payload readiness and --docker-publish. The proposed Rust gateway would replace that with -netdev stream over a Unix domain socket carrying length-prefixed Ethernet frames, so this spike must verify exact QEMU syntax, framing, connection lifecycle, and how the gateway coexists with microvm virtio-net-device.

## Design

Build or script a minimal Rust or diagnostic endpoint that listens on a Unix socket, accepts QEMU stream traffic, decodes the 4-byte big-endian frame length plus Ethernet frame bytes, and records enough packet evidence to prove guest virtio-net traffic is reaching userspace. Test against the current microvm appliance where possible. Document exact QEMU command changes from the validated microvm command, expected guest kernel behavior, error handling when the gateway is absent, and any QEMU version constraints. Include a decision on whether smoltcp is the right TCP/IP core or whether a smaller staged parser is enough for early milestones.

## Acceptance Criteria

A design note or ticket note documents commands tested, observed frame format, QEMU version behavior, guest boot/network observations, blocker list, and the recommended implementation path. Follow-up tickets are adjusted if stream-netdev framing or lifecycle differs from the assumed design.


## Notes

**2026-05-13T10:33:59Z**

Spike documented in docker/vmnet-stream-spike.md. Local QEMU is 10.2.2 and advertises stream netdev. Unix stream syntax requires addr.type=unix. Rust frontend should own the listener and QEMU should use server=off,addr.type=unix,addr.path=/vmnet.sock,reconnect-ms=250. Upstream QEMU net/stream.c confirms 4-byte big-endian length prefix before each Ethernet frame. Live guest packet capture is intentionally left for wra-iknn because it needs a reusable endpoint and QEMU launch integration.
