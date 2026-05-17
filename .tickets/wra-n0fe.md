---
id: wra-n0fe
status: open
deps: [wra-9glk, wra-r070, wra-rjt4]
links: [wra-zqci, wra-jkeg, wra-xcvq, wra-yl7i]
created: 2026-05-17T10:19:05Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, supervisor]
---
# Option 1: introduce Tokio application edge and supervisor skeleton

Introduce one Tokio runtime at the agentvm application edge and add an async supervisor skeleton without converting vmnet internals yet. The supervisor should be the owner of process and service lifecycle rather than launch.rs spawning detached threads coordinated by AtomicBool.

## Design

Use Tokio features only where needed: rt-multi-thread, macros, net, process, signal, sync, time, io-util, fs as required. Add a LaunchSupervisor or equivalent that owns task handles, cancellation, readiness channels, state writing, shutdown policy, and typed task results. Keep composed-fs on dedicated blocking threads or spawn_blocking initially.

## Acceptance Criteria

The binary can enter an async run path, supervisor APIs exist with typed errors and cancellation, no long-lived service is newly detached, and existing launch behavior is still covered by tests before deeper conversion.


## Notes

**2026-05-17T10:20:56Z**

Coordinate with wra-yl7i. The supervisor skeleton should be designed as the eventual backend for a project-local control socket: VM lifecycle, payload sessions, diagnostics, status/log events, shutdown semantics, and client attach/detach should have explicit ownership boundaries even if the socket protocol is implemented later.

**2026-05-17T10:28:44Z**

Async-boundary refinement: supervisor should own composed-fs/config-fs as blocking services under Tokio lifecycle supervision, not force their internals onto Tokio. Use dedicated blocking threads/spawn_blocking or process ownership with readiness/failure/shutdown reporting. Avoid running blocking filesystem request work on core Tokio workers.
