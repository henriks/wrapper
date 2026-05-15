---
id: wra-ieju
status: closed
deps: [wra-cr8p]
links: []
created: 2026-05-15T19:43:18Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-bjlw
tags: [config, codex, pi, mounts]
---
# Update setup recipes to write explicit share templates

Setup recipes should produce concrete config share templates instead of relying primarily on broad magic tool_state flags. Current setup_tool codex/pi writes tool_state booleans, and runtime_manifest.rs maps CODEX_TOOL_STATE_DIRS/PI_TOOL_STATE_DIRS to host home tool-state mounts. Desired direction: recipe-generated .sandbox/config.json should explicitly model the shares needed by that tool, including shadows for volatile child paths where appropriate, while keeping runtime mount behavior generic.

Context from discussion: Codex may need host ~/.codex writable, but volatile cleanup-sensitive paths such as tmp should be project-local shadows. The config should express that as normal shares: host_path ~/.codex, guest_path ~/.codex, access rw, shadows ["tmp"] (or narrower paths if later evidence says so). Pi should likewise use explicit share entries for ~/.pi or other required state. Avoid adding Codex-specific path handling in runtime_manifest.rs or launch policy.

## Design

After the generic shadow behavior lands, update WrapperSandboxConfig::setup_tool / SetupTool helpers so generated config includes explicit ConfigShare records for tool state. Decide whether tool_state remains for manual config or is removed/collapsed as part of simplifying the pre-user config format. Update tests that assert setup-tool config output and launch args. If removing tool_state, update docs and runtime_manifest.rs callers accordingly rather than preserving compatibility code.

## Acceptance Criteria

agentvm --setup-tool codex writes config with explicit share template(s), including project-local shadow(s) for volatile state. agentvm --setup-tool pi writes explicit share template(s). Tests assert generated config shape and launch args. No runtime code contains Codex-specific child path filters; tool-specific choices live in recipe config generation only.


## Notes

**2026-05-15T20:37:15Z**

Iteration 2 progress: setup recipes now generate explicit ConfigShare entries instead of enabling tool_state booleans. Codex recipe writes an optional rw ~/.codex share with shadows ["tmp"] so volatile state backs onto .sandbox/root/<guest path>; Pi recipe writes an optional rw ~/.pi share. Updated setup-tool tests and config-json/wrapper UX docs for explicit recipe shares. Targeted validation passed: ./vm-frontend/validate.sh docs; cargo test --manifest-path vm-frontend/Cargo.toml --offline setup_tool; cargo test --manifest-path vm-frontend/Cargo.toml --offline config_share_shadow; cargo test --manifest-path vm-frontend/Cargo.toml --offline legacy_config. Remaining before closing: remove or collapse old manual tool_state/runtime Codex/Pi path handling so tool-specific path choices live only in recipe config generation.

**2026-05-15T20:42:17Z**

Completed implementation: removed tool_state from config schema/WrapperSandboxConfig, removed --tool-state CLI parsing and Codex/Pi ToolStateMounts/runtime_manifest path lists, and updated TUI config cycling to apply setup recipe share templates. Runtime no longer contains Codex/Pi-specific child path filters; tool-specific share choices live in SetupTool::config_shares. Codex setup emits optional rw ~/.codex plus shadows ["tmp"]; Pi emits optional rw ~/.pi. Targeted validation passed: ./vm-frontend/validate.sh docs; cargo test --manifest-path vm-frontend/Cargo.toml --offline setup_tool; cargo test --manifest-path vm-frontend/Cargo.toml --offline config_share_shadow; cargo test --manifest-path vm-frontend/Cargo.toml --offline legacy_config; cargo test --manifest-path vm-frontend/Cargo.toml --offline config_editor_model_edits_main_config_fields_before_save; cargo test --manifest-path vm-frontend/Cargo.toml --offline frontend_parses_payload_guest_share_options; cargo test --manifest-path vm-frontend/Cargo.toml --offline guest_runtime_mounts.
