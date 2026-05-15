---
id: wra-j7lp
status: closed
deps: []
links: []
created: 2026-05-15T21:49:32Z
type: task
priority: 0
assignee: Henrik Saksela
tags: [tests, live, codex, setup-tool, persistence]
---
# Add live setup-tool bootstrap and persistence regression tests

The Codex restart failure escaped because existing required validation only ran generic self-test payloads; it did not exercise wrapper setup_tool bootstrap, npm/TLS registry traffic, payload-exit VM shutdown, or no-network relaunch from persisted tool state. Add live validation paths that verify these specific contracts.

## Design

Add a live setup-tool validation tier to vm-frontend/validate.sh and include the Codex path in required. The path should create an isolated project with config.json setup_tool=codex and default_command args=[--version], run agentvm wrapper to bootstrap and execute codex --version over public egress/TLS MITM, then relaunch the same project with --no-net to prove the tool install persisted and was not truncated by shutdown. Verify output contains codex-cli and @openai/codex-linux-x64/package.json is non-empty/valid. Keep optional knobs for tool/project if needed.

## Acceptance Criteria

./vm-frontend/validate.sh required exercises Codex setup-tool install and no-net persisted restart. A broken npm/TLS bootstrap, missing setup-tool script, or post-payload truncation fails the gate. Documentation usage lists the new live tier. Required validation passes.


## Notes

**2026-05-15T21:51:36Z**

Live scenario ./vm-frontend/validate.sh live-setup-tools passed. It installed @openai/codex in a fresh isolated project, no-net restarted successfully with codex-cli 0.130.0, and verified @openai/codex-linux-x64/package.json remained non-empty valid JSON at 511 bytes.

**2026-05-15T21:52:07Z**

Required validation passed after adding live-setup-tools to the gate: ./vm-frontend/validate.sh required completed successfully. The new required gate now runs Codex setup-tool bootstrap, no-net persisted restart, and optional package metadata verification.
