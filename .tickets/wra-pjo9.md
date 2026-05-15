---
id: wra-pjo9
status: open
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

