---
id: wra-1ro6
status: closed
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

**2026-04-02T12:41:16Z**

Investigation plan:
1. Reproduce matrix. Build a minimal matrix that isolates the trigger: (a) no wrapped session, (b) wrapped VM shell only, (c) wrapped Codex active, (d) two wrapped Codex sessions in same project, (e) wrapped Codex sessions in different projects. For each case, record whether a fresh external Codex session can still run simple host commands like pwd/ls/python3 -c.
2. External observer session. Run the reproduction from a separate controlling session that never enters codex-wrap itself. That observer should poll host state while wrapped Codex is active: ps output for codex/qemu/virtiofsd, /proc limits, open file counts, cwd accessibility, stat of /usr/bin/bash and the repo root, and whether process creation failures appear outside this repo too.
3. Scope check: repo-local vs global. Repeat a simple exec from another unrelated directory while wrapped Codex is active. If host exec fails globally, the bug is likely outside sandbox-wrap or at least outside project-local runtime state. If it only fails in this repo/workspace, focus on self-hosting or workspace mutation hazards.
4. Session-scoped vs user-scoped. Test whether the failure is limited to the affected Codex session or also hits a completely separate terminal process owned by the same user. This distinguishes 'this agent session got poisoned' from 'wrapped Codex changes user-visible host state'.
5. Capture exact host error provenance. When the failure reproduces, gather strace or equivalent around a trivial exec (for example spawning /usr/bin/bash -c pwd) from the unaffected observer session. The aim is to see which path lookup or syscall returns ENOENT and whether the missing object is really /usr/bin/bash, the working directory, the interpreter, or something else.
6. Validate current lock assumptions. The current project flock already serializes same-project VM startup. Confirm whether the reproducer still happens with only one wrapped VM in the project. If yes, stop chasing same-project run-dir deletion races.
7. Validate self-hosting hypothesis. The proxy helper now uses a runtime snapshot of sandbox-wrap. Re-run the reproducer after that mitigation and compare. If the symptom remains unchanged, self-hosting is not the primary cause.
8. Check resource exhaustion hypotheses. While wrapped Codex is active, inspect user process counts, open-file limits, pty usage, tmpfs usage, and port allocations. The low-level ENOENT may be masking a resource exhaustion or sandbox/namespace failure in the surrounding platform.
9. Narrow what Codex activity matters. Compare wrapped guest shell, wrapped noninteractive guest command, wrapped Codex idle, and wrapped Codex actively editing/streaming. If only active Codex triggers the issue, instrument around stdio, PTY, and nested process-launch paths rather than VM boot alone.
10. Decide on mitigation class. Based on findings, either (a) fix a concrete wrapper bug, (b) harden with immutable runtime snapshots and stricter helper isolation, or (c) document an external platform limitation and add a clear fail-fast guard for unsupported concurrency modes.
Success criteria for the investigation phase: produce a short reproduction recipe and identify whether the fault boundary is inside sandbox-wrap, inside wrapped Codex behavior, or in the surrounding execution platform. Only then decide whether to keep pursuing full concurrent-session support or explicitly constrain it.

**2026-04-02T19:00:09Z**

New hypothesis from user: the mounted ~/.codex directory may be the interference vector. Current code to verify: TOOLS["codex"]["host_home_mounts"] includes .codex, and DockerVmManager.build_guest_shares() maps host ~/.codex into the guest HOME under <project>/.sandbox/home/.codex as a read-write virtio-fs share. If the outer Codex session and wrapped guest Codex both rely on the same host ~/.codex state concurrently, that is a concrete shared mutable surface worth isolating in the repro matrix.

**2026-05-14T18:15:18Z**

Resolved by the Rust-only launcher pivot rather than by preserving the old Python path. The original high-signal hazards were: active sessions executing a mutable working-tree sandbox-wrap, helper subprocesses launched from that mutable Python file, and ambiguous same-project VM overlap. The current implementation deletes the Python sandbox-wrap, moves launcher execution into vm-frontend, holds a project-scoped flock at .sandbox/docker-vm/lock for the VM lifetime, rejects same-project concurrent launches clearly, and provides wrapper mode through the Rust binary/argv0 instead of a mutable Python helper. Live self-test and direct launch validation show no lingering QEMU after payload exit. Remaining shared surfaces are explicit VM-era guest shares (for example --gh, --aws, ~/.docker, and selected tool state as implemented by wra-eh7c), not implicit Bubblewrap/home replay. If future interference appears, it should be tracked as a new Rust guest-share/auth-state bug with a concrete reproducer; the old self-hosted Python wrapper failure mode no longer exists.
