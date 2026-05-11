---
id: wra-11dm
status: open
deps: [wra-ek03]
links: []
created: 2026-05-11T20:44:16Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-3o6e
tags: [qemu, microvm, virtiofs, validation, docs]
---
# Validate and document microvm plus composed filesystem integration

Run and document the full microvm + composed fs integration baseline. This is the gate before changing defaults.

## Design

Test boot, Docker readiness, payload readiness, networking, hostfwd, --docker-publish, Docker bind mounts, auth/state sharing, user --ro/--rw, cleanup behavior, and logs. Compare behavior against the q35 + composed fs validation notes.

## Acceptance Criteria

Validation results are documented; any blockers have follow-up tickets; microvm + composed fs is explicitly approved or not approved for default use based on evidence.

