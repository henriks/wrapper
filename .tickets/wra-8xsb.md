---
id: wra-8xsb
status: closed
deps: [wra-zlsn, wra-qdte, wra-rleu, wra-3gdr, wra-fsv6, wra-bcvj]
links: [wra-0a0r, wra-w8ea]
created: 2026-05-18T05:37:21Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-9m5h
tags: [cleanup, composed-fs, virtiofs]
---
# Consolidate composed-fs structure while preserving bounded blocking model

composed-fs has grown around correctness fixes for namespace traversal, inode identity, POSIX locks, readdir/cache behavior, and vhost request handling. The cleanup goal is not to make the filesystem async; it is to reduce module size and duplicate helper logic while keeping vhost/filesystem request execution bounded and blocking.

## Design

Fold in the direction from wra-0a0r and coordinate with wra-3gdr, wra-zlsn, wra-fsv6, wra-bcvj, wra-rleu, and wra-qdte. Extract modules only when the extraction deletes duplication or makes ownership/confinement boundaries clear. Keep async out of the filesystem core unless the vhost backend itself changes architecture. Add focused tests/fuzzing for arbitrary paths, symlinks, and lock inputs touched by the cleanup.

## Acceptance Criteria

composed-fs no longer relies on monolithic helper clusters where smaller ownership-bound modules would delete duplication; correctness tickets remain linked and are not bypassed; vhost request execution stays bounded blocking; relevant tests/fuzz targets and required validation are recorded before close.


## Notes

**2026-05-18T08:11:47Z**

Iteration 21 cleanup reflection: still blocked by composed-fs correctness prerequisites wra-zlsn, wra-qdte, wra-rleu, wra-3gdr, wra-fsv6, and wra-bcvj. Do not start structural cleanup until those correctness boundaries are settled; preserve the bounded blocking vhost/filesystem model and only extract modules when it deletes duplication or clarifies ownership/confinement.

**2026-05-18T09:25:59Z**

Prerequisite wra-bcvj is closed. POSIX lock owner semantics now share locks across handles by dev/ino/owner, with two-handle regression and multi-handle lock proptest coverage. Remaining blockers are wra-zlsn, wra-qdte, wra-rleu, wra-3gdr, and wra-fsv6.

**2026-05-18T09:32:39Z**

Prerequisite wra-qdte is closed. composed-fs now requires openat2 for host-relative traversal and fails closed on ENOSYS/EINVAL, with live required validation at /tmp/pi-bash-8bea82967fc804e6.log. Remaining blockers: wra-zlsn, wra-rleu, wra-3gdr, wra-fsv6.

**2026-05-18T09:38:00Z**

Prerequisite wra-rleu is closed. readlink/access now reuse confined parent resolution rather than direct full relative-path readlinkat/faccessat, with parent-symlink replacement regressions and required validation at /tmp/pi-bash-970b7b9667685e04.log. Remaining blockers: wra-zlsn, wra-3gdr, wra-fsv6.

**2026-05-18T09:46:56Z**

Prerequisite wra-zlsn is closed. Host-backed nodes now carry dev/ino identity and validate stale cached paths before use; replacement host paths get distinct backend inodes and stale old inodes fail ESTALE. Required validation passed at /tmp/pi-bash-5e4e798ccd25797a.log. Remaining blockers: wra-3gdr and wra-fsv6.

**2026-05-18T09:54:35Z**

Prerequisite wra-3gdr has partial progress: cold zero-lookup host/shadow file nodes are pruned with required validation at /tmp/pi-bash-bc7d08901a951a94.log. wra-3gdr remains a blocker because bounded per-call readdir work is still unfinished.

**2026-05-18T10:02:24Z**

Prerequisite wra-3gdr is closed. Readdir is now bounded by FUSE buffer-derived entry budget and cold zero-lookup host/shadow inode cache entries are pruned; required validation passed at /tmp/pi-bash-674f45bf0d9348fc.log. Remaining blocker before composed-fs cleanup is wra-fsv6.

**2026-05-18T10:06:49Z**

Prerequisite wra-fsv6 is closed. SETLKW no longer performs indefinite F_OFD_SETLKW on vhost request workers; it uses bounded nonblocking OFD lock retry and returns a conflict after the wait cap. Required validation passed at /tmp/pi-bash-f220349929bb6059.log. All listed blockers for wra-8xsb are now closed, so composed-fs structural cleanup can start.

**2026-05-18T10:06:56Z**

Iteration 43 start: all prerequisite correctness blockers are now closed (wra-bcvj, wra-qdte, wra-rleu, wra-zlsn, wra-3gdr, wra-fsv6). Begin with an audit for deletion/consolidation opportunities only after the traversal, identity, readdir/cache, lock, and bounded worker semantics are stable. Keep composed-fs as a bounded blocking backend; do not introduce async filesystem core code.

**2026-05-18T10:07:13Z**

Iteration 43 audit after blocker closure: composed-fs/src/lib.rs is still ~7.5k lines. The clearest consolidation target is the host traversal/metadata helper cluster (open_beneath/openat2, parent/leaf helpers, readlink/access/stat/mkdir/mknod/unlink/rename/link/symlink, xattr/statvfs/setattr helpers) plus host identity types. Extracting that into a focused host_ops module should clarify the openat2 confinement boundary without changing the bounded blocking vhost model. Keep lock wait code in the filesystem core or a small lock helper until structural extraction proves it deletes duplication.

**2026-05-18T10:13:27Z**

Iteration 44 cleanup slice: extracted the host traversal/metadata helper cluster from composed-fs/src/lib.rs into new composed-fs/src/host_ops.rs. The new module owns the openat2-confined host operation boundary (mount-root open, open/stat/readlink/access beneath, parent/leaf namespace mutations, setattr/xattr/statvfs helpers, bounded readdir helpers, and lock-file reopen/OFD lock helpers) while ComposedFs remains a bounded blocking FileSystem implementation. Full composed-fs tests and ./vm-frontend/validate.sh required passed at /tmp/pi-bash-993c468a1e57703b.log.

**2026-05-18T12:28:26Z**

Follow-up wra-w8ea is closed with required validation. It continued the composed-fs cleanup by splitting namespace runtime into namespace.rs, moving large tests into tests.rs, and sharing test_support helpers with fuzz_harness/tests while preserving the bounded blocking backend.
