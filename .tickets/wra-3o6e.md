---
id: wra-3o6e
status: open
deps: [wra-h5hv, wra-jyn1]
links: []
created: 2026-05-11T20:43:04Z
type: epic
priority: 1
assignee: Henrik Saksela
parent: wra-myui
tags: [qemu, microvm, rollout]
---
# Switch QEMU backend to microvm and retire per-share exports

Rollout epic for adding the documented microvm QEMU command branch, validating it with the composed filesystem, measuring improvement, switching defaults, and retiring the old per-share export path after a fallback window.

## Design

Do not switch to microvm until composed fs works on q35 and the microvm spike has documented the exact device model. Preserve fallback until acceptance criteria are met.

## Acceptance Criteria

microvm + composed fs passes integration tests; performance measurements are documented; defaults are switched intentionally; old per-share code is removed or explicitly retained with rationale.
