---
id: wra-cr8p
status: open
deps: [wra-71xw]
links: []
created: 2026-05-15T19:43:09Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-bjlw
tags: [filesystem, mounts, config]
---
# Make share shadows generic rw substitutions under .sandbox/root

Update runtime shadow semantics to match the agreed generic model. Current implementation only permits shadows on rw parent shares, requires the low-level parent to match a rw share, and stores backing under .sandbox/share-shadows/share-NNNN/<relative>. Desired behavior: parent share may be ro or rw; every shadow is a read-write project-local substitution; backing path is derived from the full guest shadow path under <project>/.sandbox/root/<guest path without leading slash>.

Relevant code: vm-frontend/src/main.rs apply_configured_launch_defaults, config_share_shadow_backing_path, parse_guest_path_share_shadow, runtime_mounts, shadow_backing_is_project_local, tests around config_share_shadow_*; runtime_manifest nested mount behavior already supports more-specific child mounts.

## Design

Remove the parent-rw validation. Compute the full guest shadow path as parent guest path joined with the normalized relative shadow. Derive backing with a generic project-local guest path helper: project/.sandbox/root/<guest path components>. Ensure the helper rejects non-absolute/escaping guest paths. Keep the hidden internal --share-shadow mechanism if useful, but it should carry enough information to create a rw nested mount regardless of parent access. Emit shadow mounts after parent mounts so the more-specific child wins.

## Acceptance Criteria

Runtime accepts shadows on ro and rw parent shares. Shadow RuntimeMount entries are always readonly=false and source_class UserRw. Backing directories are created under .sandbox/root/<full guest shadow path>. Tests assert ro parent + rw shadow, rw parent + rw shadow, backing path shape, and parent-before-shadow ordering.

