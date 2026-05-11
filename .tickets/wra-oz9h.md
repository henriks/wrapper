---
id: wra-oz9h
status: open
deps: [wra-0bcu, wra-d8nv, wra-udix]
links: []
created: 2026-05-11T20:43:59Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-h5hv
tags: [q35, virtiofs, validation, docs]
---
# Validate and document q35 plus composed filesystem integration

Run and document the q35 + composed fs integration baseline before any microvm production switch. This is the gate that proves filesystem semantics independently from machine-type changes.

## Design

Test boot, Docker readiness, payload readiness, project path identity, Docker bind mounts, Codex/Copilot auth and state, --ro, --rw, --no-net, --docker-publish, logs, cleanup, and fallback behavior. Add ticket notes for any details that affect later microvm rollout.

## Acceptance Criteria

Validation results are documented in plan.md, an adjacent docker design note, or ticket notes; failures have follow-up tickets; q35 + composed fs is considered usable enough to proceed to microvm implementation.

