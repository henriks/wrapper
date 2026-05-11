---
id: wra-h5hv
status: open
deps: [wra-umuv]
links: []
created: 2026-05-11T20:43:04Z
type: epic
priority: 1
assignee: Henrik Saksela
parent: wra-myui
tags: [guest-init, q35, virtiofs, integration]
---
# Integrate composed filesystem into guest and q35 path

Integration epic for using the composed export on the current q35 backend before switching machine type. This isolates filesystem semantics from microvm boot/device changes.

## Design

Replace share-per-tag guest reconstruction with a bind manifest derived from the composed export. Keep the old per-share implementation available as fallback until q35 + composed fs passes integration tests.

## Acceptance Criteria

q35 + composed fs boots, reaches Docker and payload readiness, preserves auth/state sharing and user --ro/--rw behavior, and has documented validation results.

