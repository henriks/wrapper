---
id: wra-09v9
status: open
deps: [wra-9glk]
links: []
created: 2026-05-17T10:18:43Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, cli]
---
# Option 1: split vm-frontend main into responsibility modules

Split vm-frontend/src/main.rs into focused modules before deep async work. Current main.rs mixes CLI dispatch, wrapper UX, .sandbox/config.json structs and migration, launch argument construction, self-test orchestration, TLS CA bootstrap, runtime mounts, local smoke servers, credential extraction, and payload execution.

## Design

Suggested modules: cli.rs for clap definitions and command enum, config.rs for .sandbox/config.json parsing/writing/validation/migration, wrapper.rs for wrapper command assembly, mounts.rs or env.rs for runtime mounts and auth/share resolution, self_test.rs for live checks, tls_bootstrap.rs for MITM CA setup, and paths/model modules as needed. Preserve config-file compatibility only. Update vm-frontend/config-json.md and tests in the same change if config semantics move or change.

## Acceptance Criteria

main.rs becomes a thin binary entry and dispatch layer, config parsing remains documented and tested, behavior is pinned with focused tests, and unrelated runtime code is not refactored in the same ticket.

