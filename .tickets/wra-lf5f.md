---
id: wra-lf5f
status: closed
deps: [wra-0bmi]
links: []
created: 2026-05-14T20:41:05Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-zewf
tags: [mounts, tool-state, ui, tui, vm-frontend]
---
# Model Codex tool-state mounts without argv0 entrypoints

Model Codex tool-state/config mounts independently from hard-coded wrapper entrypoints.

Context:
- Epic: `wra-zewf` Interactive TUI wrapper for VM payload sessions.
- Direction update: enabling Codex in the TUI should add the correct Codex directories as rw mounts in the sandbox configuration.
- The TUI setup flow should include relevant Codex tool-state/config directories as rw mounts when Codex is selected, without relying on `codex-wrap` argv[0] behavior.
- Existing runtime mount logic lives around `runtime_mounts`, `guest_runtime_mounts`, `GuestTool`, and wrapper flag translation in `vm-frontend/src/main.rs` / `vm-frontend/src/runtime_manifest.rs`. Current requirements mention selected tool state and `codex|copilot` tool selection; this should be revisited.

Goal:
Replace hard-coded executable-name assumptions with an explicit Codex tool-state mount model suitable for the TUI initialization dialog and explicit CLI automation.

## Design

Represent Codex tool-state mounts as data/config choices rather than argv[0] entrypoint behavior. Decide which Codex directories are required defaults, which are optional, and how they should be exposed as rw mounts. The model should not add a Copilot-specific entrypoint.

## Acceptance Criteria

- Codex tool-state mount choices are represented independently from `codex-wrap` / `copilot-wrap` executable-name logic.
- Enabling Codex in the TUI causes the expected Codex directories to be included as rw mounts in the generated sandbox config.
- Runtime manifest generation can consume the selected optional mounts.
- Requirements/docs are updated to remove misleading `codex-wrap` / `copilot-wrap` entrypoint language while preserving Codex state setup through the TUI.
- Tests cover manifest/config generation for Codex tool-state mount choices.

## Notes

**2026-05-14T21:06:47Z**

Implemented explicit Codex tool-state mount selection in `vm-frontend/src/runtime_manifest.rs`: added `ToolStateMounts`, `CODEX_TOOL_STATE_DIRS`, and a test proving `.codex` can be mounted without selecting a `GuestTool`. `--tool codex` remains compatible by deriving `ToolStateMounts::from_guest_tool(policy.tool)`. Documented Codex rw state setup in `requirements.md`. Verified with `cargo test --manifest-path vm-frontend/Cargo.toml --offline`.
