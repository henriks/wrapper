---
id: wra-3o6e
status: closed
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

## Notes

**2026-05-13T06:29:28Z**

Input from wra-9xru: q35 composed and microvm composed are functionally validated, and local readiness measurement supports proceeding with the default-switch patch while keeping an explicit fallback. Use docker/vm-startup-measurement.md criteria for the switch: preserve readiness within 20% or 1s of q35 fallback, keep Docker/socket and filesystem smoke behavior, keep composed host processes <=4 and QEMU devices <=6, and do not remove the old per-share path until wra-t79v.

**2026-05-13T06:35:11Z**

Switched Docker VM defaults intentionally: --docker now resolves to microvm + composed fs; --docker-machine q35 keeps q35 composed available; --docker-legacy-per-share-fs keeps the old q35 per-share fallback for the fallback window. Updated docs in docker/microvm-qemu-branch.md, docker/microvm-composed-validation.md, docker/composed-fs-q35.md, docker/vm-startup-measurement.md, and plan.md. Validation run after the switch: py_compile; docker/check-qemu-command-shape.py; --help; default --docker --no-net state asserted machine_type=microvm and composed_fs=enabled; --docker-legacy-per-share-fs asserted machine_type=q35 and composed_fs=disabled; --docker-machine q35 asserted machine_type=q35 and composed_fs=enabled.
