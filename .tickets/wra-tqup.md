---
id: wra-tqup
status: open
deps: [wra-4bi6]
links: []
created: 2026-05-15T08:12:26Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-293p
---
# Make agentvm the primary user entrypoint

Replace implementation-centered wrapper UX with a primary agentvm command centered on the 80% use case: run the configured project sandbox from the current directory. Keep low-level agentvm-frontend subcommands for development/debugging, but normal docs/help should point users at agentvm. Relevant code/docs: run_cli and print_wrapper_usage in vm-frontend/src/main.rs; requirements.md; docker/runtime-contract.md; validation-workflow.md; build/install packaging if present. Decide whether agentvm with no subcommand starts the configured project, and how explicit subcommands like init/config/status/reset/shell map to the same backend.

## Acceptance Criteria

There is a documented primary agentvm UX. Normal first-run and subsequent-run examples use agentvm, not agentvm-frontend wrap. Low-level frontend commands remain accessible but are clearly marked internal/debug. Tests cover the selected agentvm command dispatch and backwards-compatible behavior for existing wrap usage if retained.

