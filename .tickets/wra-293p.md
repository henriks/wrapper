---
id: wra-293p
status: closed
deps: []
links: []
created: 2026-05-15T08:07:53Z
type: epic
priority: 1
assignee: Henrik Saksela
---
# Reframe wrapper contracts around coherent UX

Revisit the wrapper/runtime contracts from the user's perspective rather than from the VM implementation outward. The goal is a coherent, convenient tool contract: one obvious way to start a project sandbox, no repeated prompts after setup, clear command override semantics, predictable project-local state, understandable network/auth/share choices, and actionable errors/status. Relevant docs/code: requirements.md, docker/runtime-contract.md, vm-frontend/tui-design.md, vm-frontend/validation-workflow.md, vm-frontend/src/main.rs wrapper parsing/run_wrapper, vm-frontend/src/runtime_manifest.rs guest share/tool-state handling.

## Acceptance Criteria

A user-facing UX contract is documented. CLI/help/docs use consistent terms. Implementation tickets exist or are completed for any contract-code mismatches. The default interactive path, configured-project path, command override path, reset path, and non-TTY/plain path are each covered by tests or validation notes.


## Notes

**2026-05-15T08:12:01Z**

Product decisions from UX review: the primary user-facing command should be agentvm, centered on the 80% default use case. config.json should be the durable source of truth for basically every configurable sandbox option: entrypoint/default command, directory mounts, network mode, network host allowlists, auth sharing, etc. CLI switches should be one-run overrides and should not persist into config.json by default. Command override should probably use the post--- command position rather than a separate --command flag as the canonical UX. End goal: the TUI can view/edit all relevant config.json fields. Reset/inspect/status contracts are accepted as important but need detailed design.

**2026-05-15T08:19:02Z**

Additional UX decision: replace the current launch-time --tool concept with explicit setup recipes. A setup recipe such as --setup-tool codex or --setup-tool pi should mutate config.json intentionally: set the default entrypoint, required writable tool-state shares, install/bootstrap recipe, environment defaults, and any known package metadata. Current --tool should become a compatibility/transient override or be retired after migration. Candidate recipes include codex and pi for @mariozechner/pi-coding-agent.

**2026-05-15T08:35:09Z**

Implemented the UX epic end to end. Added wrapper-ux-contract.md, made agentvm the primary entrypoint, expanded config.json to schema_version 2 for durable command/network/auth/share/port/setup state, converted post--- to payload command override, added --setup-tool codex|pi recipes, added an initial TUI config editor via --config, aligned docs, and added/updated tests. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check passed. Remaining product hardening is richer free-form TUI editing, but the coherent config-driven contract is implemented and covered.
