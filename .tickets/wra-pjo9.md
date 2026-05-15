---
id: wra-pjo9
status: closed
deps: [wra-huvu]
links: []
created: 2026-05-15T08:12:26Z
type: feature
priority: 2
assignee: Henrik Saksela
parent: wra-293p
---
# Add TUI config viewer and editor for sandbox config

Make the TUI the long-term way to view and edit relevant .sandbox/config.json fields: default entrypoint/command, network mode, whitelisted hosts/domains/IPs, directory mounts, auth sharing, published ports, and reset/reinitialize choices. This should build on the wrapper prompt/focus architecture in vm-frontend/src/tui.rs and the expanded config schema. It should avoid forcing users to hand-edit JSON for common tasks while keeping config.json transparent and editable.

## Acceptance Criteria

The TUI can display current project config and edit the main fields without launching a separate payload flow unexpectedly. Changes are written to config.json only after explicit acceptance. Tests cover config model edits where practical; manual TUI smoke covers editing default command, network mode/allowlist, and mounts.


## Notes

**2026-05-15T08:34:35Z**

Added an initial TUI config editor reachable with agentvm --config. It loads existing config or a Codex default, displays the main fields, and writes config.json only on explicit save. The current editor supports keyboard edits for default command/setup recipe cycle, network mode including allowlist seed, GitHub auth, a sample rw share, and a sample published port. A model-level unit test covers these edits. This is a first coherent editor surface; richer free-form text editing for arbitrary paths/domains/profiles can build on the same schema/model.
