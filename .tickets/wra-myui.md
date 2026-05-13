---
id: wra-myui
status: closed
deps: []
links: []
created: 2026-05-11T20:42:47Z
type: epic
priority: 1
assignee: Henrik Saksela
tags: [qemu, microvm, virtiofs, filesystem]
---
# Migrate QEMU backend to microvm with composed virtio-fs

Umbrella epic for migrating the current q35 QEMU backend to microvm while replacing device-per-share virtiofsd usage with one composed virtio-fs backend. The composed filesystem is a critical user-experience requirement: host paths must appear at natural guest absolute paths without one VM device and host daemon per share. Work should follow plan.md and update plan.md, adjacent docker design docs, or ticket notes with outcomes before tickets are closed.

## Design

Do not build duplicate staged-tree or symlink-projection implementations in parallel. Use spikes to document constraints and decisions, then implement the composed backend behind a fallback path before switching defaults.

## Acceptance Criteria

All child epics are closed; q35 + composed fs and microvm + composed fs pass integration tests; startup/readiness measurements are documented; old per-share path has either been removed after a fallback window or has a documented remaining reason.


## Notes

**2026-05-13T06:35:44Z**

Umbrella outcome: composed virtio-fs backend is implemented and tested; q35 composed validation is documented; microvm composed validation is documented; startup/readiness measurement is documented; --docker now defaults to microvm composed mode. The old q35 per-share path remains intentionally behind --docker-legacy-per-share-fs for the fallback window, tracked by wra-t79v.
