---
id: wra-wi7x
status: open
deps: [wra-ee8x, wra-bmhk]
links: []
created: 2026-03-27T21:22:04Z
type: feature
priority: 0
assignee: Henrik Saksela
parent: wra-hggq
tags: [docker, vm, sandbox-wrap]
---
# Replace direct Docker socket mounting with VM-backed --docker behavior

Update sandbox-wrap so --docker starts the project VM before entering bwrap, mounts only the project-local Docker socket into the sandbox, and ensures teardown when the sandbox exits.

## Design

Scope:
- remove direct host socket discovery/mount behavior from the --docker path
- start and verify the VM before entering bwrap
- mount the project-local Unix socket into the sandbox at /run/docker.sock and /var/run/docker.sock
- preserve ~/.docker config mounting if still needed for client auth
- ensure wrapper-driven teardown on normal exit and signal exit paths
- enforce Linux/KVM-only support with clear preflight errors

## Acceptance Criteria

--docker works without exposing the host daemon directly.

The sandbox sees only the project-local socket.

Exiting the sandbox also terminates the VM.

Unsupported hosts fail early with a clear message.

