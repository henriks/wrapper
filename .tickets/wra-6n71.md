---
id: wra-6n71
status: open
deps: [wra-09v9]
links: []
created: 2026-05-17T10:20:02Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, self-test, cli, deletion]
---
# Option 1: move live self-tests out of production CLI

Move sprawling live self-test and shell/Python script generation out of the production CLI surface. The review identified run_self_test and validation harness code in vm-frontend/src/main.rs as bloat that obscures launch and runtime behavior.

## Design

Create an integration harness crate, xtask-style binary, or dedicated test support module for live scenarios. Production agentvm should launch and manage VMs, not embed large validation scripts. Keep validate.sh behavior intact or update it explicitly. Preserve live-smoke and setup-tool scenarios required by project validation.

## Acceptance Criteria

Production CLI no longer owns self-test script generation, validation entrypoints still work, live scenario code has focused ownership, and docs/scripts are updated for the new location.

