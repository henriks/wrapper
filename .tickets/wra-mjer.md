---
id: wra-mjer
status: open
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

