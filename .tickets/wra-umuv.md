---
id: wra-umuv
status: open
deps: [wra-jyn1]
links: []
created: 2026-05-11T20:43:04Z
type: epic
priority: 1
assignee: Henrik Saksela
parent: wra-myui
tags: [virtiofs, filesystem, rust]
---
# Build composed virtio-fs backend

Implementation epic for the repo-local composed virtio-fs backend. This is critical-path work for user experience: it replaces per-share virtiofsd daemons and exports one manifest-defined namespace rooted at / for guest bind reconstruction.

## Design

Reuse upstream virtiofsd at the protocol boundary if the spike validates that seam. Implement manifest parsing, namespace construction, safe fd-relative host resolution, inode/lookup bookkeeping, readonly enforcement, and the v1 operation surface behind a fallback to the old per-share path.

## Acceptance Criteria

The backend passes correctness and adversarial filesystem tests, serves the current q35 VM path behind a feature flag, and documents semantic compromises or unsupported operations.

