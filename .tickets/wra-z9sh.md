---
id: wra-z9sh
status: in_progress
deps: [wra-p7m4, wra-52t3, wra-9ida]
links: []
created: 2026-05-13T10:23:03Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, network, qemu, control]
---
# Preserve host-to-guest Docker payload and published-port access over vmnet

Replacing QEMU user-mode networking removes the current hostfwd mechanism used for the project-local Docker socket proxy, guest payload control, and --docker-publish. Design and implement the Rust frontend equivalent so the host can still reach required guest services through the userspace stream gateway or an explicitly chosen alternative control channel.

## Design

Start from current behavior: sandbox-wrap allocates host Docker/payload ports, QEMU user networking forwards them into guest TCP ports, and the host proxy exposes .sandbox/docker-vm/run/docker.sock. The Rust gateway path must preserve Docker readiness checks, payload launch/control, signal/resize/stdio behavior, and published localhost ports. Options include implementing host-originated TCP sessions in the gateway TCP stack, adding a dedicated virtio-serial/control channel, or keeping a narrow separate QEMU device if justified. Document the chosen design and why it is not duplicative.

## Acceptance Criteria

The Rust frontend has a working plan and implementation for host-to-guest Docker and payload control without QEMU user-mode hostfwd. --docker-publish semantics are either preserved or explicitly replaced. Validation covers Docker _ping, payload ping/run/exit, and one published localhost port.


## Notes

**2026-05-13T10:33:59Z**

wra-lxhx confirmed the Rust vmnet pivot removes QEMU user-mode hostfwd. This ticket must restore host-to-guest Docker, payload control, and --docker-publish by frontend-owned host loopback listeners that proxy into the guest over the userspace TCP gateway. Do not retain QEMU user networking as a parallel legacy path.

**2026-05-13T10:38:58Z**

wra-52t3 intentionally keeps QEMU usernet/hostfwd out of the Rust command shape. Host-to-guest Docker, payload, and published ports must be implemented as frontend-owned listeners/proxies, not by adding a second QEMU network backend.

**2026-05-13T10:40:54Z**

wra-40vh maps Docker API, payload control, and --docker-publish to frontend-owned HostListener entries. Docker/payload management listeners should remain separate from guest egress policy and should not use QEMU hostfwd.

**2026-05-13T15:35:58Z**

Dependency moved behind wra-9ida. Host-to-guest Docker/payload/published-port access should build on the Rust stream launch instead of blocking it. Do not duplicate QEMU usernet/hostfwd; document the chosen ingress/control design and validation outcomes when implemented.

**2026-05-13T16:03:01Z**

wra-p7m4 now has a validation-only guest HTTP smoke hook, so outbound HTTP vmnet validation no longer needs to wait for the broader host-to-guest Docker/payload ingress implementation. This ticket should still restore real frontend-owned host listeners for Docker, payload, and published ports after p7m4 closes.

**2026-05-13T16:12:49Z**

Started after wra-p7m4 closed. Existing design points to frontend-owned loopback HostListener entries for Docker, payload, and published ports. The p7m4 local smoke upstream proves the gateway can map a guest destination to a host loopback TcpStream for validation, but production host-to-guest ingress still needs its own listener/session implementation rather than QEMU usernet/hostfwd.
