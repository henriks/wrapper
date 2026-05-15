---
id: wra-huvu
status: open
deps: [wra-4bi6]
links: []
created: 2026-05-15T08:12:26Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-293p
---
# Expand config.json into the full sandbox configuration schema

Design and implement .sandbox/config.json as the durable source of truth for configurable project sandbox behavior. It should cover entrypoint/default command, directory mounts, network mode, whitelisted network hosts/domains/IPs, auth sharing (GitHub/AWS), published ports, TUI preference if needed, and any other project-level options. CLI switches should act as one-run overrides and should not persist by default. Current implementation only stores schema_version, codex_enabled, and default_command in WrapperSandboxConfig in vm-frontend/src/main.rs.

## Acceptance Criteria

A versioned config schema is documented and implemented. Wrapper launch builds its launch config from config.json plus one-run CLI overrides. Existing flags remain overrides unless an explicit config-edit command is used. Tests cover config loading defaults, schema validation, network allowlists, mounts, auth options, default command, and CLI override precedence.

