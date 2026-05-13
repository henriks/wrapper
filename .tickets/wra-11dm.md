---
id: wra-11dm
status: closed
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


## Notes

**2026-05-12T20:54:46Z**

Dependency insight from wra-30di: microvm + composed fs validation should reuse the smoke commands from docker/filesystem-semantics-baseline.md after q35 validation passes, especially Docker bind mounts, payload/tool state writes, readonly failures, and open-after-rename/unlink behavior.

**2026-05-13T06:16:36Z**

Input from q35 composed-fs validation: before microvm validation, ensure the composed backend process remains alive after daemon.start(listener) by retaining the daemon.wait() behavior. Repeat the q35 smoke coverage on microvm: payload/Docker readiness, project path identity and host writeback, --ro rejection, --rw persistence, Docker bind mounts from $PWD, --docker-publish hostfwd, and fallback/cleanup expectations as applicable. q35 validation details are recorded in docker/composed-fs-q35.md.

**2026-05-13T06:21:24Z**

wra-ek03 added the non-default launch switch for validation: --docker --docker-composed-fs --docker-machine microvm. The branch deliberately rejects microvm without --docker-composed-fs. Before boot testing, run python3 docker/check-qemu-command-shape.py to confirm q35 remains PCI/-nic and microvm remains non-PCI/-netdev. Full boot and behavior validation remains owned here.

**2026-05-13T06:25:03Z**

Completed microvm composed-fs validation. Preflight passed via python3 docker/check-qemu-command-shape.py. Integration passed: --docker --docker-composed-fs --docker-machine microvm --no-net boot/payload/Docker readiness; project cwd identity and host writeback; composed export and bind reconstruction; --ro write rejection; --rw host persistence; Docker bind mount from $PWD; --docker-publish 28082:18082 returning microvm-publish-ok from host localhost; clean shutdown removed .sandbox/docker-vm/run. Durable results are documented in docker/microvm-composed-validation.md and plan.md points to them. Remaining default-rollout gate is comparative startup/readiness measurement in wra-9xru.
