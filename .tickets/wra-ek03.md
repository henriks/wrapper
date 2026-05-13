---
id: wra-ek03
status: closed
deps: [wra-mjer, wra-oz9h]
links: []
created: 2026-05-11T20:44:16Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-3o6e
tags: [qemu, microvm]
---
# Add documented microvm QEMU launch branch

Add the microvm QEMU command branch using the exact command shape and constraints documented by the microvm spike. This should be implemented after q35 + composed fs is validated so machine-type issues are isolated.

## Design

Use virtio-mmio-compatible devices for rootfs, Docker data disk, rng, net, and composed fs. Preserve user-mode networking and hostfwd behavior if the spike validated it. Keep the branch gated until integration validation passes.

## Acceptance Criteria

The microvm branch is implemented behind a non-default switch; command construction is covered enough to prevent regressions; deviations from the spike are documented with reasons.


## Notes

**2026-05-11T21:22:59Z**

Dependency insight from wra-mjer: implement microvm branch from docker/microvm-spike.md. Required shape: -machine microvm,acpi=off,memory-backend=mem,isa-serial=on; -enable-kvm; likely -cpu host; use virtio-blk-device, virtio-rng-device, vhost-user-fs-device, and -netdev user plus virtio-net-device. Do not use PCI devices. Keep direct kernel/initrd boot and memory-backend-memfd share=on for vhost-user-fs. Hostfwd has been validated for payload control and Docker _ping.

**2026-05-13T06:21:20Z**

Implemented gated microvm command branch behind --docker-machine microvm. The branch requires --docker-composed-fs to avoid duplicating the old per-share export model on microvm. Command construction follows docker/microvm-spike.md: microvm with acpi=off and isa-serial=on, -cpu host, virtio-blk-device, virtio-rng-device, vhost-user-fs-device, and -netdev user plus virtio-net-device. Added docker/microvm-qemu-branch.md and docker/check-qemu-command-shape.py. Validation run: python3 -m py_compile sandbox-wrap docker/check-qemu-command-shape.py; python3 docker/check-qemu-command-shape.py; ./sandbox-wrap --help.
