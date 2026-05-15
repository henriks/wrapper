---
id: wra-ndxm
status: closed
deps: [wra-jek5]
links: []
created: 2026-05-14T21:47:08Z
type: bug
priority: 0
assignee: Henrik Saksela
parent: wra-yz27
tags: [composed-fs, filesystem, concurrency]
---
# Make shared writable host paths safe for concurrent host and guest processes

Do not encode Codex-specific state semantics in the wrapper. The contract should be generic filesystem behavior: when a guest process accesses a guest-visible path that corresponds to a host path, other host processes should see ordinary filesystem effects and should not be broken by missing or surprising composed-fs semantics.

The motivating case was host and guest Codex both touching ~/.codex, but the fix must be app-agnostic. The wrapper should expose natural guest paths and make composed-fs implement the filesystem semantics required by real tools, rather than special-casing Codex directories or files.

Relevant code/docs:
- vm-frontend/src/runtime_manifest.rs generates composed-fs mount manifests.
- composed-fs/src/lib.rs implements the host-backed filesystem.
- docker/filesystem-semantics-baseline.md already identifies required development filesystem semantics, including open handles, rename/unlink, fsync, xattrs, cache policy, and lock deferrals.
- Prior Codex-specific tickets wra-brsl and wra-kst9 were closed as superseded because the wrapper must remain app-agnostic.

## Design

- Treat Codex only as one workload exercising generic FS semantics.
- Add workload/regression tests using ordinary filesystem/database operations rather than Codex-specific allowlists.
- Review composed-fs behavior for concurrent host mutation, path cache/attribute coherency, open handles, fsync/flush, rename/unlink, and error propagation.
- Any restrictions should be mount/FS capability restrictions, not app-directory policy.

## Acceptance Criteria

- No new wrapper code contains Codex-specific path allowlists or SQLite-file policies.
- Concurrent host/guest access tests cover a shared writable directory with generic operations.
- Documented composed-fs semantics are sufficient for normal tools sharing a writable directory through natural paths.
- Any unsupported filesystem feature fails explicitly or is documented as a generic mount capability limitation.


## Notes

**2026-05-14T21:51:16Z**

User requested extending the test sets for these aspects and any other missing POSIX filesystem behavior. When implementing this ticket, broaden tests beyond the motivating Codex case: cover natural home path overlays, nested mount boundaries, concurrent host/guest mutation, and additional POSIX filesystem semantics not already covered by the existing baseline.

**2026-05-14T21:57:14Z**

Expanded generic shared filesystem coverage without adding Codex-specific policy. Added composed-fs tests for natural home overlay with nested workspace/tool-state mounts and host/guest visibility through the same writable mount. Existing tests already cover rename/unlink, xattrs, readonly boundaries, symlink escape, open handles, readdir snapshots, and operation-model/property sequences. Verification: cargo test --manifest-path composed-fs/Cargo.toml --offline.
