---
id: wra-ieju
status: open
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

