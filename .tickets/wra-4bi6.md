---
id: wra-4bi6
status: closed
deps: []
links: []
created: 2026-05-15T08:08:04Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-293p
---
# Write user-facing wrapper UX contract

Produce a single UX-first contract document for the wrapper. Start from requirements.md, docker/runtime-contract.md, vm-frontend/tui-design.md, and current vm-frontend/src/main.rs behavior. Reframe contracts around user jobs: start/resume a project sandbox, run default agent, override with shell/command, configure auth/network/shares, inspect status/logs, reset state, and recover from errors. Explicitly distinguish user-facing commands from low-level frontend/debug commands. Capture decisions on terminology: project sandbox, default command, configured project, tool state, guest home, network mode, auth sharing, shares, reset.

## Acceptance Criteria

A checked-in doc defines the UX contract and identifies any existing docs that are superseded or need alignment. The doc includes primary command examples for first run, subsequent run, command override, no-network, auth sharing, extra shares, non-TTY/plain mode, and reset. It calls out unresolved product decisions instead of hiding ambiguity.


## Notes

**2026-05-15T08:22:58Z**

Added wrapper-ux-contract.md as the UX-first source of truth. It defines agentvm as the primary command, config.json as durable setup state, post--- command override semantics, one-run overrides, setup-tool recipes, TUI config editing expectations, and reset/non-TTY behavior. Existing implementation docs should align to this document rather than invent separate wrapper CLI terms.
