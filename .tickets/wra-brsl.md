---
id: wra-brsl
status: closed
deps: [wra-jek5]
links: []
created: 2026-05-14T21:45:41Z
type: bug
priority: 0
assignee: Henrik Saksela
parent: wra-yz27
tags: [codex, filesystem, state]
---
# Stop sharing the entire host ~/.codex tree read-write

The wrapper currently exposes host ~/.codex as writable tool state for guest Codex. That means host Codex and guest Codex can concurrently mutate the same directory tree, including session indexes, logs, tmp dirs, cache, auth/config, skills, and SQLite databases. This can break host Codex from the host point of view even though the host only sees normal filesystem operations from the Rust composed-fs process.

This ticket fixes the shared-state policy. Guest Codex should not get the whole host ~/.codex tree as a writable mount. Use a narrow sharing model: share only the minimum host material needed for authentication/config, preferably read-only where possible, and keep guest runtime state (sessions, logs, tmp, caches, sqlite databases, shell snapshots, plugin caches) in guest-owned/project-local backing storage.

Relevant code/docs:
- vm-frontend/src/runtime_manifest.rs: CODEX_TOOL_STATE_DIRS = [".codex"] and guest_runtime_mounts maps host_home/.codex as writable ToolState.
- requirements.md says enabling Codex exposes ~/.codex as writable tool state.
- docker/filesystem-semantics-baseline.md treats .codex as writable tool-state.
- Host ~/.codex observed contents include auth.json, config.toml, session_index.jsonl, state_5.sqlite, logs_2.sqlite, WAL/SHM files, tmp, sessions, skills, cache, and logs.

## Design

- First complete the natural guest home path work so the guest-visible Codex path is not under .sandbox.
- Define an allowlist for host Codex files that may be shared into guest Codex, with access mode per file/directory.
- Prefer guest-owned writable state for volatile Codex paths.
- Avoid broad host home exposure; this is about precise Codex state policy, not mounting the entire host home.

## Acceptance Criteria

- The generated manifest no longer contains a writable directory mount from host ~/.codex to guest ~/.codex.
- Guest Codex can authenticate/use required config without mutating host sessions/logs/tmp/cache/sqlite state.
- Host Codex can run concurrently with guest Codex without both processes writing the same Codex runtime files.
- Runtime manifest tests assert the new Codex sharing policy and access modes.
- Documentation no longer says ~/.codex is shared wholesale read-write.


## Notes

**2026-05-14T21:46:47Z**

Superseded after clarification: do not add Codex-specific sharing policy or allowlists. The wrapper should not encode Codex semantics. The corrected work is generic: expose natural paths and make shared writable directories behave like a normal filesystem, including concurrent host/guest access semantics.
