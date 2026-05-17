---
id: wra-r070
status: open
deps: [wra-09v9]
links: []
created: 2026-05-17T10:18:54Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, errors]
---
# Option 1: replace stringly errors with typed domain errors

Replace internal Result<_, String> plumbing with typed errors before introducing broad async code. Current vm-frontend paths erase source errors and context in CLI/config/launch/self-test code, while lower modules already use thiserror in places.

## Design

Introduce focused error enums such as CliError, ConfigError, SupervisorError, PayloadError, VmnetError, and keep LaunchError where appropriate. Preserve source errors, paths, command context, child status, policy context, and timeout information. Format into user-facing strings only at the binary boundary.

## Acceptance Criteria

Internal command/config/launch/payload/vmnet APIs no longer convert rich errors to String mid-stack, tests assert structured failures where practical, and user-facing output remains clear.

