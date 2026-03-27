---
id: wra-bmhk
status: open
deps: [wra-ee8x]
links: []
created: 2026-03-27T21:22:04Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-hggq
tags: [docker, vm, proxy]
---
# Expose guest Docker through a project-local host Unix socket

Provide the host-side proxy that presents a normal Unix Docker socket to the sandbox while forwarding to the guest over vsock.

## Design

Scope:
- socket path under .sandbox/docker-vm/
- per-connection forwarding into the VM vsock bridge
- readiness and liveness checks suitable for the wrapper
- clear error messages when the proxy or guest Docker is unavailable
- shutdown behavior aligned with sandbox exit so stale sockets are cleaned up

## Acceptance Criteria

A host process can talk to Docker through the project-local Unix socket during sandbox lifetime.

The socket is created during startup and removed during teardown.

Failure modes are reported clearly enough for the wrapper to abort with actionable errors.

