---
id: wra-rjt4
status: open
deps: [wra-r070]
links: []
created: 2026-05-17T10:18:59Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, observability]
---
# Option 1: add tracing for launch and runtime supervision

Add structured observability before replacing runtime internals. Current launch/runtime paths rely on println phase strings and eprintln warnings, which makes async supervision failures hard to diagnose.

## Design

Introduce tracing spans/events around command dispatch, runtime paths, socket paths, QEMU PID/status, service task start/readiness/failure/shutdown, policy decisions, payload lifecycle, and vmnet events. Keep user-facing CLI output intentional and separate from diagnostic logs.

## Acceptance Criteria

Launch and runtime services emit structured task and lifecycle events, failures include service names and causes, and tests or snapshots are updated where output changes.

