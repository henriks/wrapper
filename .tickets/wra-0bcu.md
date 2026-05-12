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


## Notes

**2026-05-12T20:57:18Z**

Dependency insight from wra-a9je: host integration should generate .sandbox/docker-vm/run/composed-fs-manifest.json for the backend and .sandbox/docker-vm/run/guest-config/composed-binds.json for guest init. Keep existing config share in v1. Validate schema, source existence, host access, protected paths, duplicate/overlap conflicts, and kind matches before starting backend or QEMU. See docker/composed-fs-manifest.md.

**2026-05-12T21:08:42Z**

Scaffold handoff from wra-saox: runtime integration should launch composed-fs/target/debug/agentvm-composed-fs in development, or the packaged agentvm-composed-fs binary once release packaging exists. The CLI already supports --manifest .sandbox/docker-vm/run/composed-fs-manifest.json --socket-path .sandbox/docker-vm/run/virtiofs.sock --tag agentvm; QEMU should connect vhost-user-fs-device to that socket/tag. Sandbox-local smoke testing showed Unix listener creation can require host privileges outside the coding sandbox.
