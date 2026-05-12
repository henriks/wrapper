---
id: wra-zgpp
status: closed
deps: []
links: []
created: 2026-05-11T20:43:19Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-jyn1
tags: [spike, virtiofs, rust, docs]
---
# Spike and document virtiofsd crate embedding feasibility

Determine whether a repo-local Rust backend can reuse upstream virtiofsd public interfaces at the protocol boundary. The expected target is using VhostUserFsBackend with a repo-owned FileSystem implementation, but the ticket must validate that against real crate APIs.

## Design

Build the smallest possible proof of concept that serves a trivial synthetic filesystem over vhost-user, or document why the public seam is not viable. Record crate version, feature flags, public API names, packaging strategy, and fallback options.

## Acceptance Criteria

The API decision is documented with evidence; either a minimal buildable proof of concept exists or a clear blocker/fallback is documented; later backend tickets have notes if their assumptions changed.


## Notes

**2026-05-12T20:48:31Z**

Spike completed. Documented outcome in docker/virtiofsd-embedding-spike.md. virtiofsd 1.13.3 exposes the needed public seam: filesystem::FileSystem, SerializableFileSystem, DirectoryIterator, and vhost_user::VhostUserFsBackendBuilder. A temporary local probe in /tmp/virtiofsd-api-probe built successfully against virtiofsd 1.13.3 with default-features=false plus vhost 0.13.0, vhost-user-backend 0.17.0, and vm-memory 0.16.x. The probe constructed VhostUserFsBackend<ProbeFs>, wrapped it in VhostUserDaemon, and created a vhost-user listener socket. Decision: proceed with repo-owned ComposedFs implementing FileSystem + SerializableFileSystem and reuse upstream virtiofsd at the protocol boundary.
