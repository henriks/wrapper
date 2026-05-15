---
id: wra-r3p1
status: closed
deps: [wra-s38i]
links: []
created: 2026-05-15T19:47:27Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-ia1a
tags: [docs, vm, persistence]
---
# Rename and document VM persistent state artifacts

Clean up names and documentation after root overlay persistence lands. Current docs and paths describe docker-data.raw as the persistent disk and .sandbox/home as HOME backing. The target docs should describe immutable lower rootfs plus project-local persistent overlay state disk, and optionally a separate Docker disk only if validation proved necessary.

Docs likely affected: vm-frontend/config-json.md, requirements.md, docker/runtime-contract.md, docker/OPERATIONS.md, docker/README.md, vm-frontend/validation-workflow.md, wrapper-ux-contract.md. Code names may also need cleanup: RuntimePaths::data_disk, ensure_data_disk, artifact manifest metadata naming, tests that mention Docker data disk.

## Design

Audit for stale phrases: docker-data.raw, Docker data disk, .sandbox/home backing, rootfs stays read-only without mentioning overlay, and any implication that Docker has unique persistence by default. Prefer names like state.raw/root-overlay-state.raw for the primary disk. Keep documentation precise about lower rootfs immutability versus overlay-root writability.

## Acceptance Criteria

Docs and code names match the final architecture. There is no stale claim that docker-data.raw is the main persistence model unless a separate Docker disk was justified. AGENTS-required config docs remain in sync. ./vm-frontend/validate.sh required passes.


## Notes

**2026-05-15T20:21:53Z**

Documentation progress: updated active runtime docs away from Docker-only persistence and .sandbox/home: requirements.md, docker/runtime-contract.md, docker/OPERATIONS.md, docker/README.md, wrapper-ux-contract.md, docker/composed-fs-manifest.md, docker/filesystem-semantics-baseline.md, and docker/root-overlay-design.md now describe state.raw as the persistent root overlay and ordinary HOME/Docker state living there. ./vm-frontend/validate.sh docs passed.

**2026-05-15T20:25:32Z**

Completion evidence: active runtime docs now describe .sandbox/docker-vm/state.raw as the persistent root overlay state disk and no longer document docker-data.raw or .sandbox/home/persistent-home as active runtime artifacts. Updated docs include requirements.md, docker/runtime-contract.md, docker/OPERATIONS.md, docker/README.md, docker/composed-fs-manifest.md, docker/filesystem-semantics-baseline.md, wrapper-ux-contract.md, and root-overlay-design target/current notes. ./vm-frontend/validate.sh docs passed.
