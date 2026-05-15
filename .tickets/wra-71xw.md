---
id: wra-71xw
status: open
deps: []
links: []
created: 2026-05-15T19:42:58Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-bjlw
tags: [config, mounts]
---
# Switch share shadow config to string arrays

Change .sandbox/config.json share shadows from objects to a plain array of strings. Current code has ConfigShareShadow { path: String } and docs show shadows: [{ "path": "tmp" }]. Desired format is shadows: ["tmp", "cache/foo"]. Config compatibility is not required; remove the object format rather than supporting both.

Relevant files: vm-frontend/src/main.rs ConfigShare/ConfigShareShadow/validate_share_shadow_path and tests; vm-frontend/config-json.md; wrapper-ux-contract.md; requirements.md; docker/runtime-contract.md.

## Design

Represent ConfigShare.shadows as Vec<String>. Reuse/adjust validate_share_shadow_path for each string. Keep validations: non-empty, relative only, no .., no =, duplicate normalized shadow paths rejected. Update tests and all docs to show the string-array format.

## Acceptance Criteria

Config parsing and serialization use shadows as string arrays. Object-form shadows are no longer documented or required. Tests cover valid string shadows and duplicate/escaping rejection.

