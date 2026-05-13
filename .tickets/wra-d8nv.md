---
id: wra-d8nv
status: closed
deps: [wra-a9je]
links: [wra-0bcu]
created: 2026-05-11T20:43:59Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-h5hv
tags: [guest-init, virtiofs, integration]
---
# Superseded: guest init composed export work folded into wra-0bcu

This ticket is superseded by `wra-0bcu`. The guest init work should be implemented together with q35 host launch wiring so the composed filesystem path is end-to-end usable behind one explicit switch.

## Design

Do not implement this as a separate host/guest intermediary. Preserve the requirements below in `wra-0bcu`: use the guest bind manifest from the schema ticket, define parent directory creation, required versus optional binds, failure behavior, and how boot config is delivered. Do not remove the current config share unless a replacement is implemented and documented.

## Acceptance Criteria

Superseded ticket is closed after its requirements are folded into `wra-0bcu`; no separate implementation should be done here.


## Notes

**2026-05-12T20:57:08Z**

Dependency insight from wra-a9je: guest init should mount composed export tag agentvm at /run/agentvm-host, then read /run/agentvm-config/composed-binds.json from the existing tiny config share. Bind entries are derived from host manifest and include composed_mountpoint, source, target, kind, required, and create_parent. Guest init should process parent-before-child, create target parents, create file placeholders for kind=file, fail required bind errors, and log/skip optional failures. It must not interpret host paths or source classes.

**2026-05-13T05:00:51Z**

Superseded by wra-0bcu after planning correction. Guest init composed-export binding should be implemented in the same ticket as q35 host composed backend wiring to avoid a host-only half-state and duplicate integration work. Preserve this ticket's guest init requirements in wra-0bcu.
