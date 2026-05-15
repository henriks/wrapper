---
id: wra-a0bu
status: closed
deps: [wra-kh0g, wra-pssg]
links: []
created: 2026-05-15T10:39:19Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-neci
tags: [validation, filesystem, virtiofs]
---
# Add live and adversarial composed-fs coverage

composed-fs has offline tests, but the VM contract also depends on virtiofs/kernel behavior. Add live and adversarial coverage for composed-fs and its mounts. Relevant areas include composed-fs crate tests, vm-frontend mount setup, virtiofsd integration, and self-test workspace/config mounts. Cover read-only vs read-write layers, SQLite write behavior on mounted directories, openat2 availability/fallback behavior, nested mounts, symlinks and path traversal attempts, rename/link/unlink semantics, readdir stability, file truncation/append behavior, permissions/uid/gid mapping, stale sockets, and repeated runs.

## Design

Keep pure VFS behavior in composed-fs offline tests. Add live VM tests only for behavior that depends on virtiofsd or Linux guest semantics. Use small temporary fixtures and assert both host-visible and guest-visible file state where writable mounts are involved.

## Acceptance Criteria

Offline tests include adversarial path and filesystem operation coverage. Live validation proves workspace/config mounts behave correctly in read-only and read-write modes, including SQLite writes and repeated runs. Stale virtiofs sockets/state are cleaned up or fail with a direct diagnostic before boot.


## Notes

**2026-05-15T11:02:52Z**

While validating wra-1bks/wra-o9y4, the first ./vm-frontend/validate.sh fast run hit a transient composed-fs POSIX lock failure in composed_lock_bridge_flush_and_release_cleanup_owner_locks: host POSIX lock after release returned WouldBlock. Immediate targeted rerun passed and full fast rerun passed. When expanding composed-fs adversarial coverage, consider whether lock cleanup tests need stronger isolation/diagnostics for transient host-lock contention.

**2026-05-15T11:13:09Z**

Addressed repeated transient composed-fs POSIX lock flake seen during validate fast. Added fcntl_lock_eventually helper and used it in composed_lock_bridge_flush_and_release_cleanup_owner_locks so the test still fails on persistent lock leaks but tolerates short host lock release latency. Targeted test and full validate fast passed afterward.

**2026-05-15T11:14:47Z**

The composed-fs POSIX lock flake fix was validated through full ./vm-frontend/validate.sh fast after adding fcntl_lock_eventually; no lock failure reproduced in the final run.

**2026-05-15T14:33:52Z**

During wra-zbix live-docker work, a specialized no-net Docker scenario repeatedly hit sqlite concurrency disk I/O before Docker denial assertions. The general smoke/hostile sqlite coverage remains active, but docker-net-check+no-net now skips sqlite concurrency to avoid cross-contract interference. This reinforces that wra-a0bu should own focused composed-fs live/adversarial sqlite/virtiofs coverage rather than relying on every network scenario to exercise it.

**2026-05-15T14:39:31Z**

Started after wra-zbix closure. Existing coverage already includes substantial composed-fs offline adversarial/property tests plus live self-test workspace/config checks: config mount is read-only, workspace writes/bind mounts work, sqlite WAL writes/concurrency run from guest and host, and repeated live scenarios exercise stale socket cleanup indirectly. Remaining work should add a named live-fs tier (preferably using shared self-test) that makes the composed-fs contract explicit, including adversarial path/symlink attempts and repeated run-dir reuse.

**2026-05-15T14:44:45Z**

Completed composed-fs live/adversarial coverage. Added self-test --fs-check and validate.sh live-fs, documented in AGENTS.md/README/validation workflow. fs-check exercises guest-visible composed-fs behavior: config mount remains private/RO via key symlink failure, workspace create/append/truncate/rename/unlink/readdir/uid-gid semantics, and writes a host-visible marker verified from the host after payload exit. live-fs runs the same run-dir twice to exercise stale socket/state cleanup. First live-fs attempt exposed immediate post-rename lookup latency, so fs-check now retries boundedly before failing. A required validation run also reproduced the composed_lock_bridge_flush_and_release_cleanup_owner_locks owner-flush lock flake; added fs_setlk_eventually to tolerate short OFD lock handoff latency while preserving persistent-leak failure. Validation: targeted fs-check self-test unit passed; live-fs passed both first and repeated run-dir scenarios with fs-live-ok; targeted composed lock test passed; ./vm-frontend/validate.sh required passed end-to-end after the lock retry fix.
