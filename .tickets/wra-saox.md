---
id: wra-saox
status: open
deps: [wra-zgpp, wra-a9je]
links: []
created: 2026-05-11T20:43:42Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-umuv
tags: [virtiofs, rust, backend]
---
# Scaffold repo-local composed virtio-fs backend

Create the repo-local Rust backend binary for the composed virtio-fs server using the API decision from the virtiofsd embedding spike. The initial binary should parse a manifest, start a vhost-user socket, expose a minimal namespace, and fit the wrapper's runtime/log layout.

## Design

Keep this behind a non-default path. Do not implement the full filesystem semantics here. The goal is a buildable backend skeleton and host process integration points that later tickets can extend.

## Acceptance Criteria

The backend builds locally; it can start and serve a trivial or minimal manifest-defined namespace; logs and socket paths are documented; any deviations from the spike decision are added as ticket notes.


## Notes

**2026-05-12T20:48:38Z**

Dependency insight from wra-zgpp: scaffold the backend using virtiofsd 1.13.3 with default-features=false, plus direct dependencies vhost 0.13.0, vhost-user-backend 0.17.0, and vm-memory 0.16.x with backend-mmap/backend-atomic. Use VhostUserFsBackendBuilder around repo-owned ComposedFs and VhostUserDaemon with GuestMemoryAtomic<GuestMemoryMmap>. See docker/virtiofsd-embedding-spike.md for the compile probe and recommended process wiring.
