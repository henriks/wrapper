---
id: wra-bjlw
status: closed
deps: []
links: []
created: 2026-05-15T19:42:52Z
type: epic
priority: 0
assignee: Henrik Saksela
tags: [config, mounts, filesystem, vm]
---
# Normalize generic share shadow configuration and recipe share templates

Track the cleanup of configured share shadows after the initial wra-u05c implementation. The intended model is generic and not Codex-specific: configured shares may declare shadowed child paths, shadows are project-local writable substitutions, and recipe configs should be real share templates rather than magic tool-state booleans.

Current code context: vm-frontend/src/main.rs has ConfigShare { host_path, guest_path, access, required, shadows: Vec<ConfigShareShadow> } where ConfigShareShadow is currently an object with { path }. It currently rejects shadows unless the parent share is rw, derives backing as .sandbox/share-shadows/share-NNNN/<relative>, and passes hidden --share-shadow PARENT_GUEST=RELATIVE=BACKING to the low-level frontend. runtime_mounts validates the shadow parent as a configured rw share and emits a nested UserRw RuntimeMount. Docs live in vm-frontend/config-json.md, requirements.md, docker/runtime-contract.md, and wrapper-ux-contract.md.

Desired model from discussion: shadows should be a simple array of strings; parent shares may be ro or rw; shadows are always read-write project-local substitutions; shadow backing is derived from the full guest shadow path under <project>/.sandbox/root/<guest path without leading slash>; setup recipes should emit explicit share template config instead of relying on broad/special tool-state flags. Also note the separate design concern: current .sandbox/home is still a special persistent-home mount and there is no persistent root overlay; do not conflate that with shadow backing.

## Design

Implement in small steps: first normalize the config file shape and validation, then change runtime backing/mount generation, then update setup recipes and docs/tests. Keep this generic; do not add Codex-specific runtime_manifest.rs path lists. Because config compatibility is not required for this pre-user app, remove the temporary shadow object format rather than supporting both forms.

## Acceptance Criteria

The config format documents shadows as string arrays. Runtime accepts shadows on both ro and rw parent shares. Shadow mounts are always rw and backed by .sandbox/root/<full guest shadow path>. Tests cover ro parent + rw shadow, rw parent + rw shadow, escaping/duplicate validation, and manifest/runtime mount ordering. Setup recipes produce explicit share template config for tool state needs. Required validation ./vm-frontend/validate.sh required passes before closing.


## Notes

**2026-05-15T20:44:26Z**

Implemented and validated generic share shadow model: config shadows are string arrays; shadows work for ro/rw parent shares; shadow mounts are always rw UserRw and backed by .sandbox/root/<full guest path>; setup recipes write explicit share templates (Codex ~/.codex with shadows ["tmp"], Pi ~/.pi); tool_state/--tool-state and runtime Codex/Pi path lists were removed. Guest HOME persistence remains root-overlay state from wra-ia1a, not special .sandbox/home. ./vm-frontend/validate.sh required passed.
