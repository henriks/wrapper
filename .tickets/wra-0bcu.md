---
id: wra-0bcu
status: open
deps: [wra-a9je, wra-vy20, wra-udix]
links: []
created: 2026-05-11T20:43:59Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-h5hv
tags: [q35, virtiofs, integration]
---
# Wire composed backend into q35 host launch behind fallback

Integrate the composed backend into the current q35 QEMU launch path behind a feature flag or equivalent non-default switch. Replace per-share virtiofsd launch only for the gated path and preserve the old path as fallback.

## Design

Update sandbox-wrap process supervision, socket/log/pid paths under .sandbox/docker-vm/run/, manifest generation, backend startup validation, and failure handling. Keep networking unchanged. Do not remove DockerVmGuestShare/per-share code in this ticket.

## Acceptance Criteria

q35 can boot with the composed backend through an explicit switch; the old per-share path still works; logs and state files are documented; startup failures fail before QEMU when manifest/backend validation fails.

