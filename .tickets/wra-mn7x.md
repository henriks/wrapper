---
id: wra-mn7x
status: closed
deps: []
links: []
created: 2026-05-15T06:47:28Z
type: task
priority: 1
assignee: Henrik Saksela
tags: [docs, virtiofs, maintenance]
---
# Document vendored virtiofsd maintenance workflow

Document why third_party/virtiofsd is vendored, what local changes it carries, and how to merge future upstream virtiofsd crate releases without dropping the FUSE lock API changes required by composed-fs POSIX byte-range locks. Relevant files: third_party/virtiofsd, composed-fs/Cargo.toml, docker/virtiofsd-embedding-spike.md, composed-fs/README.md.

## Acceptance Criteria

Docs explain vendoring rationale, local patch surface, upstream merge steps, verification commands, and what to do if upstream adds native lock request APIs.


## Notes

**2026-05-15T06:48:34Z**

Added vendoring documentation in third_party/virtiofsd/README.agentvm.md instead of docker/. The doc explains why virtiofsd is vendored, the intended small local patch surface in filesystem.rs and server.rs, how to merge a newer upstream crate release, how to handle a future upstream-native lock API, and required verification including lock property stress and live SQLite validation. Added a pointer from composed-fs/README.md. Verification: git diff --check and tk dep cycle passed.
