---
id: wra-bmhk
status: closed
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


## Notes

**2026-03-27T21:59:07Z**

Launcher primitives from wra-ee8x are now in sandbox-wrap as DockerVmManager. The proxy ticket should plug into the existing runtime layout: use .sandbox/docker-vm/run/docker.sock for the host-visible socket, .sandbox/docker-vm/run/ch-vsock.sock for Cloud Hypervisor vsock transport, and .sandbox/docker-vm/run/proxy.{pid,log} for proxy lifecycle.

**2026-03-27T22:01:35Z**

Implemented the host-side Unix socket proxy inside sandbox-wrap. Added hidden internal mode __docker-vm-proxy, per-connection forwarding from .sandbox/docker-vm/run/docker.sock into .sandbox/docker-vm/run/ch-vsock.sock with the required CONNECT <port> preamble, proxy lifecycle management in DockerVmManager, proxy.pid/proxy.log handling, and Docker readiness checks over GET /_ping via the project-local Unix socket. Verification completed: python compilation and hidden-mode CLI parsing. Verification gap: end-to-end Unix socket bind/connect testing is blocked in this execution environment because AF_UNIX bind returns EPERM here.
