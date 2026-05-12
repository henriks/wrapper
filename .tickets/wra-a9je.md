---
id: wra-a9je
status: closed
deps: [wra-30di]
links: []
created: 2026-05-11T20:43:42Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-umuv
tags: [virtiofs, manifest, guest-init, docs]
---
# Define composed mount and guest bind manifest formats

Define the host manifest consumed by ComposedFs and the guest bind manifest consumed by guest-init. This covers schema versioning, mount ids, guest absolute paths, host source paths, dir/file kind, ro/rw mode, source class, uid/gid/permission policy, bind target hints, required/optional entries, and serialization paths under .sandbox/docker-vm/run/.

## Design

Use plan.md as the starting point. Specify overlap and conflict rules precisely: duplicate guest targets, file-over-dir, dir-over-file, more-specific overrides, nested ro inside rw, user --ro/--rw precedence, protected internal runtime paths, missing required sources, and optional auth/tool-state sources.

## Acceptance Criteria

The manifest schemas and conflict rules are documented; examples cover workspace, .codex, .docker, gh config, system certs, file mounts, user --ro, and user --rw; backend and guest-init implementation tickets have enough detail to start without rediscovery.


## Notes

**2026-05-12T20:54:11Z**

Dependency insight from wra-30di: manifest schema must encode source class and readonly/writable policy strongly enough for backend enforcement, especially nested ro inside rw. It must support first-class file mounts and protected internal runtime paths. Synthetic parents must be manifest-derived only; creating arbitrary new files under synthetic parents should fail unless inside a writable mounted subtree. See docker/filesystem-semantics-baseline.md.

**2026-05-12T20:56:57Z**

Manifest schema ticket completed. Documented host manifest and guest bind manifest in docker/composed-fs-manifest.md and summarized paths in plan.md. V1 host manifest path: .sandbox/docker-vm/run/composed-fs-manifest.json. V1 guest bind manifest path: .sandbox/docker-vm/run/guest-config/composed-binds.json. V1 intentionally keeps the tiny config share for bind metadata until a replacement boot-config mechanism is designed.
