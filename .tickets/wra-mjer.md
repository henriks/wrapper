---
id: wra-mjer
status: closed
deps: []
links: []
created: 2026-05-11T20:43:19Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-jyn1
tags: [spike, qemu, microvm, docs]
---
# Spike and document microvm boot/device feasibility

Prove the exact QEMU microvm command shape before production changes. Use the current appliance/rootfs where possible and test rootfs block, Docker data block, rng, virtio-mmio networking, hostfwd behavior, and one ordinary upstream virtiofsd export. This ticket is primarily complete when the outcome is documented, not when code is merged.

## Design

Do not implement the production composed filesystem here. Capture exact commands, kernel config or module requirements, observed device limits, user-mode networking behavior, hostfwd behavior, and any blockers. Update plan.md, a docker design note, or this ticket's notes with the results.

## Acceptance Criteria

A documented working microvm command exists, or a documented blocker and alternate route exists; required kernel/QEMU constraints are recorded; follow-up tickets are updated with any changed assumptions.


## Notes

**2026-05-11T21:22:51Z**

Spike completed. Documented outcome in docker/microvm-spike.md. Key result: current appliance boots under QEMU 10.2.2 microvm with KVM when using -machine microvm,acpi=off,memory-backend=mem,isa-serial=on, non-PCI virtio devices, direct kernel/initrd boot, and memory-backend-memfd share=on. With ACPI left enabled, QEMU did not inject virtio_mmio.device= arguments into the kernel cmdline, so the guest could not discover non-PCI devices. With acpi=off, QEMU injected six virtio_mmio.device entries and the guest booted, mounted rootfs/docker data, mounted both virtiofs shares, configured virtio_net, started dockerd, socket bridge, and payload server. Hostfwd was validated: payload control returned K/ok and Docker _ping returned HTTP 200 OK.
