---
id: wra-1ro6
status: open
deps: []
links: []
created: 2026-04-02T12:13:22Z
type: bug
priority: 0
assignee: Henrik Saksela
parent: wra-oszw
tags: [vm, concurrency, stability]
---
# Investigate concurrent-session interference around codex-wrap

Observed failure mode: while another codex-wrap session was active in the same repo/workspace, this Codex session's local tool layer began failing to spawn even simple host commands with low-level process-launch errors (for example, '/usr/bin/bash ...' rejected with 'No such file or directory'). The failures persisted even after the wrapped VM session exited, which suggests more than a simple same-project run-dir collision. Relevant code/context: sandbox-wrap already takes a per-project flock in DockerVmManager.acquire_lock() and removes .sandbox/docker-vm/run only after that lock is held, so same-project VM overlap should already be serialized. codex-wrap in this repo resolves to sandbox-wrap, so concurrent sessions are literally exercising the same launcher code while the repo itself may also be changing. This bug needs to establish whether the interference is caused by wrapper runtime state, concurrent mutation of the launcher/workspace, shared host resources, or something outside the repo-facing code. Preserve the exact symptom and any future reproducer commands in notes.

## Design

Start by distinguishing same-project wrapper overlap from broader shared-workspace interference. Verify what the existing project lock actually protects and what remains shared across sessions: repo files, symlinked launch entrypoints, guest HOME state, host temp paths, localhost forwards, qemu/virtiofsd processes, and any helper subprocesses. Aim for a minimal reproducer with two sessions. If the root cause is outside sandbox-wrap, document that clearly and consider mitigations in the wrapper anyway: stronger fail-fast checks, immutable launcher handoff, or a concurrency-safe execution model.

## Acceptance Criteria

We have a reproducible explanation for the interference, or a bounded set of root-cause candidates with instrumentation proving where it happens. The repo contains either a fix or an explicit, defended concurrency model describing which combinations of sessions are supported. If unsupported combinations remain, the wrapper fails fast with a clear message instead of entering a corrupt or ambiguous state.


## Notes

**2026-04-02T12:14:22Z**

Current high-signal hypothesis: self-hosting is part of the risk surface here. In this repo, codex-wrap resolves directly to sandbox-wrap in the working tree, and DockerVmManager.start_proxy() launches a helper subprocess by pointing sys.executable at repo_root/sandbox-wrap. That means concurrent sessions can be editing the same launcher source file that active wrapped sessions and helper subprocesses depend on. Even if this does not fully explain the host exec-layer failures, it is a real concurrency hazard. A likely mitigation is to snapshot the launcher into immutable runtime state (for example under .sandbox/docker-vm/run/) at VM startup and have helper subprocesses use that copied path instead of the mutable working-tree file.

**2026-04-02T12:15:05Z**

Implemented one immediate mitigation in sandbox-wrap: at VM startup, the runtime now snapshots sandbox-wrap into .sandbox/docker-vm/run/sandbox-wrap.snapshot and uses that immutable copy for the __docker-vm-proxy helper subprocess instead of repo_root/sandbox-wrap. This does not fully prove the earlier host exec-layer failures were caused by self-hosting, but it removes one concrete concurrency hazard where active wrapped sessions depended on a launcher file being edited in the working tree.
