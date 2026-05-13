---
id: wra-9xru
status: closed
deps: [wra-oz9h, wra-11dm]
links: []
created: 2026-05-11T20:44:16Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-3o6e
tags: [performance, rollout, docs]
---
# Measure startup impact and decide default rollout

Measure startup and readiness impact for the current q35 many-share path, q35 + composed fs, and microvm + composed fs. Use the data to decide when to switch defaults.

## Design

Measure host launch-to-QEMU start, host launch-to-Docker ready, host launch-to-payload ready, host process count, and QEMU device count. Document acceptance thresholds or rationale for switching despite tradeoffs.

## Acceptance Criteria

Measurements and rollout decision are documented; default switch criteria are explicit; follow-up tickets exist for unresolved performance or reliability issues.


## Notes

**2026-05-13T06:16:39Z**

Input from q35 composed-fs validation: use the documented q35 composed smoke in docker/composed-fs-q35.md as the functional baseline before measuring startup/readiness. Measure current fallback q35, q35 composed, and later microvm composed with equivalent payload readiness, Docker readiness, and --docker-publish checks so timing differences are not confounded by different validation scopes.

**2026-05-13T06:25:09Z**

Input from wra-11dm: microvm composed-fs smoke validation passed on 2026-05-13. Measurement can now compare q35 fallback, q35 composed, and microvm composed using equivalent checks. The microvm validation command shape is --docker --docker-composed-fs --docker-machine microvm; results are documented in docker/microvm-composed-validation.md.

**2026-05-13T06:29:24Z**

Completed lightweight startup/readiness measurement using state timings added to sandbox-wrap. Results documented in docker/vm-startup-measurement.md: q35 fallback payload_ready 2725.2 ms, 6 host processes, 8 QEMU devices; q35 composed payload_ready 2535.8 ms, 4 host processes, 6 QEMU devices; microvm composed payload_ready 2863.7 ms, 4 host processes, 6 QEMU devices. Decision: proceed with microvm composed default-switch work, retain explicit q35/per-share fallback until fallback-window removal. Validation run: python3 -m py_compile sandbox-wrap docker/check-qemu-command-shape.py; python3 docker/check-qemu-command-shape.py.
