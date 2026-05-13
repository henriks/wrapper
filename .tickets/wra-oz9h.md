---
id: wra-oz9h
status: in_progress
deps: [wra-0bcu, wra-udix]
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


## Notes

**2026-05-12T20:54:34Z**

Dependency insight from wra-30di: q35 + composed fs validation should run the smoke commands from docker/filesystem-semantics-baseline.md, including shell/path basics, open-after-rename/unlink, readonly path write failure, git status/rev-parse/diff, package/tool state writes under HOME, Docker bind mount from PWD, Codex/Copilot state visibility, GitHub config readonly behavior when --gh is used, and xattr smoke where host supports xattrs.

**2026-05-13T05:00:59Z**

Dependency correction: wra-d8nv is superseded by wra-0bcu, which now owns both q35 host wiring and guest init bind-manifest consumption. Validation should depend on wra-0bcu as the single end-to-end composed q35 implementation, plus backend test coverage already closed in wra-udix.

**2026-05-13T05:13:39Z**

Started validation. Current environment is uid 1000, while docker/build-appliance.sh explicitly requires root and rebuilds docker/build plus docker/out. Because docker/guest-init.sh changed for composed mode, q35 composed-fs boot validation requires rebuilding docker/out/initrd.img first; existing docker/out artifacts still contain the old guest init. Preflight checks completed: sandbox-wrap help exposes --docker-composed-fs; python3 -m py_compile sandbox-wrap, sh -n docker/guest-init.sh, cargo build/test --manifest-path composed-fs/Cargo.toml --offline all pass; manifest-generation probe writes host and bind manifests. Full q35 boot validation is blocked until appliance rebuild can be run with root/network permissions.
