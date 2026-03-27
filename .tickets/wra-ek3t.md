---
id: wra-ek3t
status: open
deps: [wra-wi7x]
links: []
created: 2026-03-27T21:22:04Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-hggq
tags: [docker, vm, docs]
---
# Add reset semantics, cleanup guarantees, and docs for Docker VM mode

Finish the feature with explicit teardown behavior, reset behavior, and user-facing documentation.

## Design

Scope:
- ensure normal sandbox exit tears down the VM and removes transient runtime files
- ensure --reset removes all VM state under .sandbox/, including the sparse Docker disk
- document prerequisites: Linux, KVM, Cloud Hypervisor, virtiofsd
- document .sandbox/docker-vm/ layout and what is safe to delete
- document start-on-launch and stop-on-exit behavior
- document common failure modes and recovery steps

## Acceptance Criteria

Docs match actual behavior.

Exit and reset behavior are explicit and tested manually.

Ticket notes include operational troubleshooting for broken VM state.

