---
id: wra-mur9
status: closed
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


## Notes

**2026-05-15T20:42:49Z**

Docs/coverage progress: removed active docs references to tool_state, --tool-state, object-form shadows, .sandbox/share-shadows, and rw-parent-only shadow language across the active config/requirements/runtime/UX docs. requirements.md now says Codex/Pi setup writes explicit optional shares, and vm-frontend/config-json.md no longer documents tool_state. ./vm-frontend/validate.sh docs passed. Remaining before close: final full validation gate after any remaining cleanup.

**2026-05-15T20:44:26Z**

Final validation/docs evidence: active docs no longer mention tool_state, --tool-state, object-form shadows, .sandbox/share-shadows, or rw-parent-only shadow rules. Tests cover string-array shadows, object-form rejection, duplicate/escaping validation, ro parent + writable shadow, rw parent ordering/backing, setup recipe share output, TUI config editing with recipe shares, and runtime mount generation. ./vm-frontend/validate.sh required passed, including docs drift, formatting, composed-fs and vm-frontend offline tests, offline guest service tests, fuzz target compilation, and live-smoke.
