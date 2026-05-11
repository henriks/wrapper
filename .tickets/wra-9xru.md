---
id: wra-9xru
status: open
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

