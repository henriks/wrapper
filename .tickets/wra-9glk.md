---
id: wra-9glk
status: open
deps: []
links: []
created: 2026-05-17T10:18:36Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, workspace]
---
# Option 1: establish root workspace and shared crate boundaries

Create a real root Cargo workspace for vm-frontend, guest-service, composed-fs, and third_party/virtiofsd so shared crates can be extracted cleanly. Centralize dependency versions where sensible. This unlocks agentvm-payload-protocol and other library splits without path/version drift.

## Design

Keep this mechanical and narrow. Do not refactor behavior here. Ensure existing package commands still work, including vm-frontend/fuzz. Be careful with third_party/virtiofsd membership and any tooling that currently assumes per-crate Cargo.toml files.

## Acceptance Criteria

cargo metadata works from the repo root, existing per-crate builds/tests still run, no behavior changes are introduced, and validation docs or scripts are updated if workspace invocation changes.

