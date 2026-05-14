---
id: wra-70mh
status: closed
deps: [wra-ukjo, wra-lf5f]
links: []
created: 2026-05-14T20:40:24Z
type: chore
priority: 1
assignee: Henrik Saksela
parent: wra-zewf
tags: [cli, cleanup, vm-frontend]
---
# Remove codex-wrap and copilot-wrap argv0 entrypoint logic

Remove the legacy program-name entrypoint behavior for `codex-wrap` and `copilot-wrap`.

Context:
- Epic: `wra-zewf` Interactive TUI wrapper for VM payload sessions.
- Direction update from planning: drop `codex-wrap` and `copilot-wrap` entrypoint logic completely. A new sandbox should use an explicit start path, with the TUI able to ask what should be initialized.
- Current logic is in `vm-frontend/src/main.rs`: `run_cli` calls `is_wrapper_program(&program)` and `run_wrapper(program, args)`; wrapper parsing also infers the selected tool from the invoked program unless `--tool` is supplied.
- Requirements currently mention `codex-wrap` and `copilot-wrap` as symlinks/aliases. Docs must be updated when this behavior is removed.

Scope:
- Remove argv[0]-based wrapper/tool selection.
- Remove or update tests that assert `codex-wrap` / `copilot-wrap` behavior.
- Update usage text and docs to point at the explicit sandbox/TUI start flow.
- Preserve any still-supported explicit tool/payload selection through the new initialization model or explicit flags, as decided by the design ticket.

## Design

Do this after the TUI initialization/start path exists so there is a clear replacement for users. Avoid leaving hidden compatibility aliases that continue selecting behavior by executable name.

## Acceptance Criteria

- `agentvm-frontend` no longer changes behavior based on being invoked as `codex-wrap` or `copilot-wrap`.
- Tests confirm argv[0] no longer selects wrapper/tool behavior.
- Documentation and usage output no longer instruct users to install or invoke `codex-wrap` / `copilot-wrap` aliases.
- The supported way to start an interactive sandbox is documented.
- Any retained explicit tool/payload selection is independent of executable name.


## Notes

**2026-05-14T20:41:50Z**

Superseded by later clarification: removal should include both `codex-wrap` and `copilot-wrap` argv[0] entrypoint/tool-inference behavior. Do not replace that with executable-name inference. Codex setup should move to the TUI initialization path, where enabling Codex adds the correct Codex state/config rw mounts.

**2026-05-14T21:24:27Z**

Removed `codex-wrap` / `copilot-wrap` argv0 entrypoint logic from `vm-frontend/src/main.rs`: `run_cli` now only recognizes explicit subcommands, wrapper parsing no longer infers tools from executable names, and usage points at `agentvm-frontend wrap`. Deleted the tracked `codex-wrap` and `copilot-wrap` symlinks. Updated `requirements.md` and `vm-frontend/tui-design.md`. Added tests proving argv0 no longer selects wrapper/tool behavior. Verified with `cargo test --manifest-path vm-frontend/Cargo.toml --offline`.
