---
id: wra-mur9
status: open
deps: [wra-q5lm]
links: []
created: 2026-05-15T19:43:34Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-bjlw
tags: [docs, testing, config]
---
# Update shadow configuration documentation and validation coverage

Bring documentation and validation coverage fully in line with the final generic share shadow model. This should happen after config syntax, runtime backing semantics, and recipe templates are updated.

Docs to update/check: vm-frontend/config-json.md is the canonical config format reference; AGENTS.md requires it to stay in sync; requirements.md, docker/runtime-contract.md, wrapper-ux-contract.md, docker/OPERATIONS.md, and any validation docs that mention .sandbox/home/tool-state/share behavior may need updates.

## Design

Audit docs for stale object-form shadows, rw-parent-only language, .sandbox/share-shadows paths, and misleading .sandbox/home or docker-data persistence claims. Add or update tests around config parsing/serialization and runtime mount generation so future drift is caught. Consider adding a lightweight doc drift assertion if there is a stable phrase worth checking.

## Acceptance Criteria

All docs describe shadows as string arrays, parent ro/rw support, always-rw project-local shadows, and .sandbox/root backing. Docs accurately distinguish docker-data.raw from any guest root/home persistence. Required validation ./vm-frontend/validate.sh required passes.

