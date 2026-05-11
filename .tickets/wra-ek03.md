---
id: wra-ek03
status: open
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

