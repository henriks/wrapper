---
id: wra-iyae
status: closed
deps: []
links: []
created: 2026-03-27T21:22:04Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-hggq
tags: [docker, vm, sandbox]
---
# Define Docker VM runtime layout and lifecycle contract

Specify the concrete host-side runtime model used by sandbox-wrap for a project-scoped, per-sandbox-lifetime Docker VM.

## Design

Decide and document:
- directory layout under .sandbox/docker-vm/
- file names for sockets, PID/state, logs, and metadata
- VM states and transitions: absent, starting, running, stopping, failed
- startup point: before entering bwrap for a --docker sandbox launch
- shutdown point: on normal sandbox exit, parent death, and signal-driven termination
- health check for Docker readiness
- locking strategy to prevent duplicate VM startups for the same sandbox invocation
- failure and cleanup behavior after partial startup

This ticket should remove lifecycle ambiguity for all downstream work.

## Acceptance Criteria

A short design doc or ticket notes define the runtime contract completely enough that remaining tickets do not need to make lifecycle decisions.

The contract explicitly says the VM is not persistent across separate sandbox runs.

The contract explicitly says --reset removes all VM assets under .sandbox/docker-vm/.


## Notes

**2026-03-27T21:26:46Z**

Captured the runtime contract in docker/runtime-contract.md. Key decisions: one active --docker sandbox per project; VM lifetime is exactly the sandbox lifetime; runtime files live under .sandbox/docker-vm/run/ while docker-data.raw persists under .sandbox/docker-vm/; --reset must refuse while the project lock is held; readiness is an HTTP /_ping over the project-local Unix socket.
