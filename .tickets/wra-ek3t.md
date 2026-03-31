---
id: wra-ek3t
status: closed
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


## Notes

**2026-03-27T22:03:23Z**

Behavior is now implemented: --docker means VM-backed Docker only; the wrapper starts DockerVmManager before bwrap, mounts only the project-local socket, sets DOCKER_HOST=unix:///run/docker.sock, and tears the VM down on sandbox exit. Docs should be updated to match the Alpine-based appliance build, lock-aware --reset behavior, host prerequisites, and the project-local runtime layout under .sandbox/docker-vm/.

**2026-03-27T22:05:06Z**

Updated the written docs to match the implemented VM-backed Docker behavior. Added docker/OPERATIONS.md covering host prerequisites, runtime layout under .sandbox/docker-vm/, start/stop flow, lock-aware --reset semantics, and troubleshooting. Updated docker/README.md to point to the runtime docs, added a historical-context note to docker.md because the implementation pivoted from Debian to Alpine, and updated requirements.md so --docker now means project-local Docker VM rather than direct host socket mounting.
