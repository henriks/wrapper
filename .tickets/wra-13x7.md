---
id: wra-13x7
status: closed
deps: [wra-8xjs]
links: [wra-7xi3, wra-8xjs]
created: 2026-05-15T21:12:19Z
type: task
priority: 1
assignee: Henrik Saksela
tags: [vm, guest, tooling, validation, codex]
---
# Add live setup-tool bootstrap smoke for Codex/Pi

Add validation that the real VM setup-tool bootstrap path can install and start the requested agent CLI. Current coverage is mostly offline: vm-frontend/src/main.rs tests assert that --setup-tool codex/pi produce config, shares, and a payload script containing npm package names; vm-frontend/validate.sh live-smoke runs self-test payloads for boot/Docker/filesystem/network basics but does not install or execute @openai/codex or @mariozechner/pi-coding-agent. A live user run of agentvm --setup-tool codex in /home/hsaksela/Code/planb stalled with an active payload session: logs in .sandbox/docker-vm/run showed registry.npmjs.org metadata fetches for @openai/codex and repeated IPv6 unsupported_protocol denials, with no clear success/failure artifact.

## Design

Add a named live validation scenario rather than hiding this in unit tests. Prefer a bounded, deterministic command that proves the CLI binary is installed and can print version/help or otherwise exit non-interactively without requiring model credentials. The scenario should exercise the same wrapper/setup-tool path users run, collect the run dir on failure, and time out with actionable diagnostics. Coordinate with wra-8xjs, which is intended to restore mise as the primary Node/tool bootstrap mechanism; this smoke should assert the intended mise-based path once that change lands.

## Acceptance Criteria

A host-live validation command exists for setup-tool bootstrap, covering at least Codex and preferably Pi. The smoke starts from fresh relevant guest state, runs through the VM path, and verifies the requested CLI becomes executable and exits non-interactively. Failure output points to run-dir logs and distinguishes install timeout, network/DNS/TLS failure, missing npm/mise, and CLI startup failure. Documentation in vm-frontend/validation-workflow.md and/or validation-matrix.md describes when to run it. The required gate policy is updated if this should become mandatory; otherwise document why it remains named-live only.


## Notes

**2026-05-15T21:51:36Z**

Added required live Codex setup-tool bootstrap/persistence scenario under new ticket wra-j7lp. It writes isolated setup_tool=codex config with default args [--version], runs wrapper through real npm/TLS MITM install, relaunches --no-net from persisted state, and verifies optional package metadata. Pi and mise-specific assertions remain for this ticket after wra-8xjs restores mise bootstrap.

**2026-05-15T21:53:19Z**

Closing after split. Codex setup-tool bootstrap/persistence is implemented and mandatory in required via wra-j7lp/live-setup-tools. Remaining Pi/mise-specific coverage moved to wra-5rrw, dependent on wra-8xjs, because it should wait for mise bootstrap.
