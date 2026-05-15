---
id: wra-28gl
status: open
deps: [wra-tqup, wra-huvu, wra-o8pc]
links: []
created: 2026-05-15T08:08:15Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-293p
---
# Align wrapper CLI with UX contract

After the UX contract is written, update the wrapper CLI surface and help text to match it. Current friction points: explicit 'agentvm-frontend wrap' is implementation-oriented; docs mention future 'start'; no friendly command alias exists; '--command CMD' appends post-'--' args but this can be surprising; low-level launch/payload flags leak concepts into wrapper help; plain mode errors still feel implementation-oriented. Relevant code: run_cli, run_wrapper, parse_wrapper_args_with_terminal, print_wrapper_usage in vm-frontend/src/main.rs.

## Acceptance Criteria

The primary user entrypoint and help text are coherent and documented. First-run, configured run, command override, and reset examples match actual parsing. Backward-compatible aliases or deprecation behavior are explicit. Tests cover the selected user entrypoint, --command behavior, -- passthrough behavior, unknown flag errors, and non-TTY/plain fallback.


## Notes

**2026-05-15T08:12:54Z**

UX decisions to align with: agentvm should be the default user-facing command centered on the 80% use case; config.json should be the durable source of truth; CLI switches are one-run overrides and should not persist by default; command override should probably use post--- syntax as the canonical path; TUI should eventually edit all relevant config.json fields.
