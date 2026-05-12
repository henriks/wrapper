---
id: wra-saox
status: closed
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

**2026-05-12T20:57:03Z**

Dependency insight from wra-a9je: backend scaffold should accept --manifest .sandbox/docker-vm/run/composed-fs-manifest.json, --socket-path .sandbox/docker-vm/run/virtiofs.sock, and --tag agentvm. It should parse schema_version=1 from docker/composed-fs-manifest.md, reject unknown fields for v1, build synthetic parents for mount guest paths, and treat the host manifest as authoritative. Guest bind manifest is separate and not consumed by backend.

**2026-05-12T21:07:37Z**

Scaffold completed. Added composed-fs/ Rust crate with agentvm-composed-fs binary, Cargo.lock, README, and a minimal fixture manifest. The binary parses schema_version=1 with deny_unknown_fields, validates basic manifest structure, builds synthetic parents and placeholder mount roots, implements init/lookup/getattr/opendir/readdir/releasedir, constructs VhostUserFsBackend via virtiofsd 1.13.3, and starts a vhost-user listener. Verified with cargo fmt and cargo build --manifest-path composed-fs/Cargo.toml. Smoke test with composed-fs/fixtures/minimal-manifest.json created /tmp/agentvm-composed-fs-smoke.sock when run with host privileges; sandbox blocks socket listener creation with EPERM. Full host-backed traversal and operation surface remain for follow-up tickets.
