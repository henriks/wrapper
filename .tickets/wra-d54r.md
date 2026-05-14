---
id: wra-d54r
status: open
deps: [wra-0bmi]
links: []
created: 2026-05-14T20:38:47Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-zewf
tags: [ui, tui, cli, vm-frontend]
---
# Add wrapper TUI mode selection and plain-stream fallback

Add CLI/runtime mode selection for wrapper TUI behavior.

Context:
- Depends on the design ticket `wra-0bmi`.
- Wrapper-mode sessions should use TUI by default when stdin/stdout are interactive TTYs.
- Users need an explicit flag to disable the TUI and keep the current plain streaming behavior.
- Non-TTY/automation use should avoid taking over the terminal and should run in plain streaming mode automatically.

Relevant code:
- `vm-frontend/src/main.rs`: `run_cli`, `run_wrapper`, `WrapperArgs`, `parse_wrapper_args`, usage text, and tests around wrapper flag translation.
- `vm-frontend/src/payload_client.rs`: existing plain streaming entrypoints stay available.

Expected behavior:
- Final flag name should match the design ticket decision, likely `--no-tui` or `--plain`.
- Wrapper mode defaults to TUI only for interactive sessions.
- Direct `launch` / `payload-client` behavior should not accidentally change unless the design explicitly calls for it.
- Existing tests for wrapper parsing should be extended to cover default TUI selection, explicit opt-out, and non-TTY fallback where practical.

## Design

This ticket can introduce a mode enum such as `WrapperUiMode::{Auto,Tui,Plain}` or similar, but should keep implementation scoped to selection/plumbing. Full rendering and event-loop work belongs to later tickets.

## Acceptance Criteria

- Wrapper argument parsing accepts the documented TUI opt-out flag.
- Interactive wrapper runs select TUI by default.
- Non-TTY runs select plain streaming automatically or according to the documented design.
- Plain streaming remains available and uses the existing payload client behavior.
- Usage/help text documents the flag.
- Unit tests cover the new mode-selection behavior.


## Notes

**2026-05-14T20:40:03Z**

Direction update from planning discussion: mode selection should not deepen the legacy `codex-wrap` / `copilot-wrap` argv[0] behavior. Account for the planned removal ticket and target the remaining explicit sandbox start path plus plain-stream fallback.
