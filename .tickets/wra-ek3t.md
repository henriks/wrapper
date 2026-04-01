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

**2026-03-31T21:55:00Z**

Documentation follow-up after adding guest networking: `docker/OPERATIONS.md` now documents the TAP + NAT model, the extra host prerequisites (`ip`, `sysctl`, and `nft` or `iptables`), the need for a live `sudo -v` session when not running as root, and the new `--docker-publish HOST:GUEST` localhost TCP forward flag. `docker/README.md` now describes the guest mounting the workspace at the original project path plus `/workspace`, and `requirements.md` was updated so the Docker VM networking behavior is part of the stated requirements.

**2026-04-01T09:16:00Z**

Documentation pivot after replacing the privileged networking path: `docker/OPERATIONS.md`, `docker/runtime-contract.md`, `docker/README.md`, and `requirements.md` now describe the QEMU backend, unprivileged user-mode networking, the project-local Docker Unix socket exposed through QEMU `hostfwd=unix:...`, and `--docker-publish HOST:GUEST` as QEMU localhost TCP forwards. The old Cloud Hypervisor/TAP/NAT/sudo prerequisites were removed from the current docs because they are no longer part of the supported implementation.
