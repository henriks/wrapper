---
id: wra-vy20
status: open
deps: [wra-46m5, wra-30di]
links: []
created: 2026-05-11T20:43:42Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-umuv
tags: [virtiofs, filesystem, readonly]
---
# Implement ComposedFs v1 operation surface and readonly enforcement

Implement the v1 FileSystem operation surface required by the documented semantics baseline, including normal file/dir I/O and readonly policy enforcement for manifest ro mounts.

## Design

Expected operations include lookup, forget, getattr, setattr, access, opendir, readdir, releasedir, open, create, release, flush, fsync, read, write, statfs, readlink, symlink, mkdir, unlink, and rename unless the baseline documents an exception. Return EROFS for mutating operations across readonly boundaries, including truncating opens and rename into/out of readonly subtrees.

## Acceptance Criteria

The required v1 operation surface works against representative guest tooling; readonly bypass attempts fail; unsupported operations are explicitly documented with rationale and user impact.

