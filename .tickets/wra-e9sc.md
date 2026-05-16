---
id: wra-e9sc
status: closed
deps: []
links: []
created: 2026-05-16T08:13:37Z
type: feature
priority: 2
assignee: Henrik Saksela
---
# Add composed-fs filtered shadow paths

Implement a generic filtering/shadowing facility in composed-fs so selected host-backed paths are hidden from the guest's view of the host filesystem and guest writes to those paths are redirected into shadow storage instead of passing through to the host. This is intended as a foundation for fail-closing unsafe host/guest shared sidecars such as SQLite WAL files (for example *-shm and possibly *-wal), while keeping the mechanism generic enough for other filtered path classes.\n\nContext: wra-ylt9 found that normal SQLite WAL across host-direct + guest-through-composed-fs can silently lose committed rows because WAL's X-shm shared-memory coherency contract is not satisfied across the mixed access paths. See sqlite-problem.md and sqlite_wal_virtiofs_safety_memo.md. One possible first step is to let composed-fs stop exposing selected host files to the guest and instead provide guest-local/shadow versions, so the guest cannot accidentally coordinate with host-direct WAL sidecars through an unsafe shared file.\n\nRelevant code/docs: composed-fs/src/lib.rs namespace lookup/readdir/open/create/unlink/rename/link/read/write paths; composed-fs manifest docs in docker/composed-fs-manifest.md; filesystem semantics baseline in docker/filesystem-semantics-baseline.md; config/share generation in vm-frontend if manifest schema needs new fields.

## Design

Design notes:\n- Add a generic filter rule model to the composed-fs manifest rather than hard-coding SQLite names in operation code. Possible rule shape: per mount or global path patterns with actions such as hide-host-and-shadow-writes.\n- Filtered host files should not appear in guest readdir/lookup as host-backed entries.\n- Guest create/open/write for a filtered path should use shadow storage, not mutate the host file.\n- Decide where shadow storage lives and how it is keyed. It must be project/run scoped, safe against path traversal, and should preserve guest semantics across open/read/write/unlink/rename as far as needed.\n- Preserve readonly boundaries: a filtered path under an ro host mount should not become writable unless explicitly allowed by the filter policy.\n- Be careful with sidecars and path aliases: rules must apply consistently to lookup, create, mknod, open, setattr, unlink, rename, link, xattr, readdir, and possibly file mounts.\n- Avoid adding special SQLite behavior in this ticket except as tests/examples. A later ticket can use this generic mechanism for SQLite WAL policy.\n- Update manifest documentation and tests in the same change if manifest/config semantics change.

## Acceptance Criteria

Acceptance criteria:\n- Manifest/config model can express at least suffix-based filters for host-backed paths, e.g. hide/shadow paths matching *-shm or *-wal under a selected mount.\n- Filtered host files are absent from guest readdir and do not resolve to the host file via lookup/open.\n- Guest writes/creates to filtered paths go to shadow storage and do not modify the host file; host contents remain unchanged in tests.\n- Guest reads of a previously shadow-written filtered path return the shadow content.\n- Tests cover lookup, readdir, create/write/read, existing host file hidden by filter, unlink/rename behavior, readonly interaction, and path traversal/alias edge cases.\n- Documentation explains the filter/shadow semantics, limitations, and intended use as a building block for unsafe sidecars such as SQLite WAL.\n- Required validation is run before closing: ./vm-frontend/validate.sh required, or any live limitation is explicitly documented.


## Notes

**2026-05-16T08:22:27Z**

Implemented generic composed-fs filter/shadow support in progress. Manifest now accepts top-level shadow_root plus filters with suffixes/action=hide-and-shadow and optional mount_id. Matching host files are hidden from lookup/readdir; guest create/write/read/unlink/rename between filtered names use shadow_root/<mount-id>/<relative-path>; ro mounts remain ro; host<->shadow rename/link crossings fail. Added unit tests for hidden host file + redirected write, readonly behavior, shadow rename/unlink, and host/shadow rename rejection. cargo test --manifest-path composed-fs/Cargo.toml --offline passed.

**2026-05-16T08:25:29Z**

Validation passed after implementation: cargo fmt --manifest-path composed-fs/Cargo.toml --check; cargo fmt --manifest-path vm-frontend/Cargo.toml --check; bash -n vm-frontend/validate.sh; cargo test --manifest-path composed-fs/Cargo.toml --offline; ./vm-frontend/validate.sh required (including live-smoke and live-setup-tools).
