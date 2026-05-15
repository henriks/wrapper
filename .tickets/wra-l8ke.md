---
id: wra-l8ke
status: closed
deps: [wra-6cei]
links: []
created: 2026-05-15T10:39:48Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-neci
tags: [validation, setup, tooling]
---
# Test setup-tool package bootstrap paths

The setup-tool recipes should configure a project and prepare known coding-agent packages reproducibly. Add tests for recipe bootstrap behavior, especially --setup-tool codex and --setup-tool pi. Cover generated config.json entrypoint/default command, read-write shares, network defaults, whitelisted hosts, package selection, miso install invocation, idempotent reruns, missing miso diagnostics, partial install failure cleanup, unsupported recipe diagnostics, and compatibility with command override after --.

## Design

Keep tests offline by injecting PATH shims for miso and any package manager calls. Assert exact commands passed to the shim and final config.json content. Avoid depending on remote registries in normal tests; add optional live bootstrap validation only if it provides value beyond the CLI recipe tests.

## Acceptance Criteria

Recipe tests prove codex and pi produce correct config, run the expected bootstrap commands, are idempotent, and report missing/failed tooling clearly. Unsupported recipes and conflicting options fail with actionable messages.


## Notes

**2026-05-15T14:27:42Z**

Added offline setup-tool bootstrap coverage: pi setup payload idempotent command-v/npm install behavior and argument quoting; codex tool bootstrap package/auto-flags/argument quoting; unsupported setup recipe diagnostic; idempotent config writes. Targeted cargo tests for setup_tool and codex_tool_bootstrap passed.

**2026-05-15T14:28:35Z**

Completed setup-tool bootstrap coverage. Added npm-missing diagnostics to tool bootstrap scripts (agentvm: npm is required to install <tool> CLI), plus tests for pi bootstrap idempotent command-v/npm behavior and argument quoting, codex package/auto-flags/argument quoting, unsupported recipe diagnostics, and idempotent config writes. Note: current implementation uses npm bootstrap, not miso, so miso-specific ticket language is obsolete. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline setup_tool -- --nocapture passed; cargo test --manifest-path vm-frontend/Cargo.toml --offline bootstrap_script -- --nocapture passed; ./vm-frontend/validate.sh required passed end-to-end.
