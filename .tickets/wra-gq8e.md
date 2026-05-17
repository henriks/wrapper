---
id: wra-gq8e
status: open
deps: [wra-n0fe]
links: []
created: 2026-05-17T10:19:17Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, docker, proxy]
---
# Option 1: rewrite docker proxy as supervised Tokio service

Rewrite vm-frontend/src/docker_proxy.rs from a detached thread-per-connection bridge into a small supervised async service. This is a clean Tokio win and should happen before the vmnet migration.

## Design

Use tokio::net::UnixListener, tokio::net::TcpStream, tokio::io::copy_bidirectional, cancellation token integration, typed errors, and a bounded connection limit. Remove per-client threads and duplicated stale socket cleanup. Integrate with the launch supervisor so failure and shutdown are visible.

## Acceptance Criteria

Docker proxy has async unit/integration tests using a fake Unix client and fake TCP upstream, supports cancellation, propagates bind/connect/copy failures through the supervisor, and no longer spawns detached copy threads.

