---
id: wra-yz27
status: closed
deps: []
links: []
created: 2026-05-14T21:45:17Z
type: epic
priority: 0
assignee: Henrik Saksela
tags: [vm, filesystem, codex]
---
# Normalize guest home and shared filesystem semantics

The VM wrapper currently mixes guest-visible paths with project-local implementation paths. In particular, guest_payload_env sets HOME to <project>/.sandbox/home, and runtime_manifest maps host ~/.codex to <project>/.sandbox/home/.codex. The desired model is that the guest sees natural absolute paths, especially the same home path as the host user (for example /home/<user>), while backing storage may still be project-local overlay/state managed by the wrapper. The guest should not need to know about .sandbox paths.

Relevant code/docs:
- vm-frontend/src/main.rs guest_payload_env sets HOME/XDG/PATH from config.project.join(".sandbox/home").
- vm-frontend/src/runtime_manifest.rs guest_runtime_mounts and home_mount place tool state under guest_home = project/.sandbox/home.
- requirements.md currently says HOME=<project>/.sandbox/home and Codex state under project guest home.
- plan.md earlier documented the natural path model: /run/agentvm-host/home/user/.codex -> /home/user/.codex.

This epic tracks bringing the implementation and docs back to the natural guest path model, plus hardening composed-fs semantics so ordinary shared writable paths work correctly for concurrent host and guest processes. Codex is a representative workload, not a source of app-specific wrapper policy.

## Acceptance Criteria

- Guest-visible home/tool paths are natural absolute paths, not .sandbox implementation paths.
- Shared writable paths behave correctly at the filesystem level for concurrent host and guest processes, without app-specific path policy.
- Docs and validation tests describe the distinction between guest-visible paths and backing storage paths.


## Notes

**2026-05-14T21:47:26Z**

Clarification: do not encode Codex-specific semantics in wrapper code. Codex should work because guest-visible paths and composed-fs semantics are correct at the filesystem level. Codex is only a workload/regression case, not a policy source.

**2026-05-14T22:01:24Z**

Epic completed. Guest-visible HOME/tool paths now use the host-natural path while .sandbox/home remains backing storage. Composed-fs gained explicit persistent-home source class support, natural-home/nested mount tests, host/guest shared writable visibility tests, and explicit POSIX lock non-advertising/EOPNOTSUPP behavior due to virtiofsd API limits. Live self-test now covers SQLite/WAL activity under guest HOME. Payloads now drop to host-mapped uid/gid and use the guest Docker TCP bridge. No Codex-specific path allowlists or SQLite policies were added.
