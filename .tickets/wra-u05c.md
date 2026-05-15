---
id: wra-u05c
status: closed
deps: [wra-wrv7]
links: [wra-brsl, wra-wrv7]
created: 2026-05-15T18:24:20Z
type: feature
priority: 0
assignee: Henrik Saksela
tags: [filesystem, vm, config, mounts]
---
# Add generic shadow mounts for writable host directory shares

Implement a generic way for a configured host directory share to keep selected guest-visible subpaths backed by project-local state instead of the host subtree, while the parent host directory remains writable. This is needed for cases like mounting host ~/.codex writable but ensuring volatile runtime directories such as tmp are not the host tmp directory. The implementation must not encode Codex-specific path knowledge in runtime_manifest.rs or launch policy. It should be expressible through the same generic host directory mounting config/model used for other user/tool shares.

Current code context: vm-frontend/src/runtime_manifest.rs has RuntimeMount, GuestShareSpec, home_mount, user_mount, and guest_runtime_mounts. It currently mounts selected tool state like host_home/.codex to guest_home/.codex as source_class ToolState. vm-frontend/src/main.rs maps .sandbox/config.json shares plus --share-ro/--share-rw into RuntimeMounts via runtime_mounts. composed-fs already supports nested manifest mount points/overlay behavior; verify behavior before relying on it.

User constraint: ~/.codex may need to remain writable for this model, but shadowing tmp is acceptable only if implemented as generic host-directory mount configuration, not Codex special sauce. Project policy also says compatibility is only for config files and we should remove/combine code rather than add one-off defensive paths.

## Design

Prefer a small config/schema extension over hard-coded tool-state exceptions. Candidate shape: a share can declare child shadow mounts, each with a guest-relative path under the share and a project-local backing directory. Generated RuntimeMounts should mount the parent normally, then add more specific nested mounts for the shadow paths. Validate that shadow guest paths stay under the parent share, use normalized relative components, and do not target protected guest paths. Backing directories should be created under .sandbox/home or another project-local state root before manifest validation.

Update docs to describe the generic mount behavior. Add tests proving the manifest contains both parent and nested shadow mounts, the nested mount host_path is project-local rather than the host child path, duplicate/escaping shadow paths are rejected, and missing optional parent behavior is well-defined. If composed-fs nested overlay ordering is insufficient, fix it generically in composed-fs with focused tests.

## Acceptance Criteria

No Codex-specific tmp/.tmp/path list is hard-coded in runtime_manifest.rs or launch policy. The feature is driven by generic share configuration and can be used for any host directory share. Runtime manifest tests cover nested shadow mount generation and validation. Documentation explains how writable parent shares and project-local child shadows compose. The required validation gate ./vm-frontend/validate.sh required passes, including live-smoke, before closing.


## Notes

**2026-05-15T18:37:48Z**

Implemented generic configured share shadows. .sandbox/config.json shares now accept a shadows array with guest-relative child paths for rw shares; launch translation creates project-local backing under .sandbox/share-shadows/share-NNNN/<relative> and passes hidden internal --share-shadow entries to the low-level frontend. runtime_mounts validates that each shadow parent matches a configured rw share, ensures backing remains under project .sandbox, creates the backing dir, and emits the nested mount after its parent so composed-fs/bind ordering makes the child shadow win. Validation rejects readonly shadows, empty/absolute/escaping paths, duplicate shadows, and = in shadow paths. No Codex-specific tmp list was added; users can model ~/.codex/tmp or any other volatile subtree through the generic share config by disabling broad tool_state if needed and declaring an explicit rw share with shadows. Docs updated in requirements.md, docker/runtime-contract.md, and wrapper-ux-contract.md. Tests added: config_share_shadow_generates_nested_project_local_mount and config_share_shadow_validation_rejects_readonly_and_escaping_paths. Validation: ./vm-frontend/validate.sh required passed, including live-smoke.

**2026-05-15T19:20:17Z**

Follow-up documentation hardening: added vm-frontend/config-json.md as the detailed .sandbox/config.json format reference, linked it from requirements.md, docker/runtime-contract.md, and wrapper-ux-contract.md, and updated AGENTS.md to require keeping that document and tests in sync with config parsing/serialization/default/migration changes. Validation rerun: ./vm-frontend/validate.sh required passed, including live-smoke.

**2026-05-15T19:43:39Z**

Reopened briefly during design discussion, but the follow-up work is now tracked under epic wra-bjlw rather than mutating this closed ticket further. User clarified the intended shadow model: shadows should be string arrays, parent shares can be ro or rw, shadows are always rw project-local substitutions, backing should be derived from the full guest path under .sandbox/root, and setup recipes should emit explicit share templates. See child tickets wra-71xw, wra-cr8p, wra-ieju, wra-q5lm, and wra-mur9.
