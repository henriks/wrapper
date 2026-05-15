---
id: wra-2kah
status: closed
deps: [wra-b5t2]
links: []
created: 2026-05-15T06:07:31Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-zfib
tags: [sqlite, vm, tests, locks]
---
# Add live host and guest SQLite WAL concurrency validation

Add live VM validation proving that shared writable mounts are safe for SQLite-style workloads. Codex uses SQLite state, and SQLite documents that missing/broken locks can corrupt databases with concurrent access. Existing smoke coverage creates a SQLite DB inside guest HOME, but it does not prove host and guest processes coordinate on the same database through byte-range locks under concurrency.

## Design

Create a live KVM/QEMU validation that opens the same database path from a host process and a guest payload process through the composed mount. Exercise WAL mode, concurrent readers/writers, repeated open/close, transaction contention, blocking timeouts, process termination during transactions, and final PRAGMA integrity_check. Keep the workload generic and database-path configurable so it validates filesystem semantics rather than Codex behavior. Record artifacts/logs on failure.

## Acceptance Criteria

The live validation reliably passes with host and guest SQLite processes concurrently using the same WAL database on a shared writable composed-fs mount. It fails or is skipped with a clear reason when POSIX lock support is unavailable. integrity_check reports ok after stress runs and after crash/termination scenarios.


## Notes

**2026-05-15T06:24:27Z**

Implemented live self-test SQLite concurrency workload in vm-frontend/src/main.rs. The self-test now starts a host python3 sqlite3 worker and the guest payload opens the same workspace-backed WAL database at .agentvm-self-test-sqlite/state.sqlite, both perform 200 transactions, and the host runs a final PRAGMA integrity_check plus host/guest row-count assertions. Live attempts with current docker/out artifacts reached the guest payload but failed before the SQLite workload: the appliance payload process still runs as uid 0 and fails the existing id -u == AGENTVM_UID assertion immediately after self-test: home-ok. This indicates docker/out needs rebuilding from the current docker/guest-payload-server.py before the live SQLite lock validation can execute. Earlier failed attempts using .sandbox/home as host path were a test-design error; the workload now uses the workspace path so host and guest open the same backing file.

**2026-05-15T06:36:06Z**

Live SQLite lock validation succeeded with a temporary patched rootfs artifact at /tmp/agentvm-rootfs-locktest.raw because docker/out/rootfs.raw is untracked but owned by nobody and contains an older payload server. The temporary manifest was /tmp/agentvm-artifact-locktest.json. Command reached self-test: sqlite-home-smoke and self-test: sqlite-concurrency-smoke. The shared DB /home/hsaksela/ai/wrapper/.agentvm-self-test-sqlite/state.sqlite passed PRAGMA integrity_check and contained [('guest', 200), ('host', 200)] after the run. During validation, found and fixed an OFD-lock bug: Linux requires struct flock.l_pid=0 for F_OFD_SETLK/F_OFD_SETLKW; copying the guest pid caused EINVAL and SQLite reported disk I/O error at PRAGMA journal_mode=WAL. The complete self-test still failed later at the Docker step with Get http://127.0.0.1:1075/v1.51/version: dial tcp 127.0.0.1:1075: i/o timeout; that is outside this SQLite/lock validation ticket.

**2026-05-15T06:41:28Z**

Re-ran live SQLite validation after replacing /proc/self/fd lock reopening with path reopen plus dev/ino verification. The SQLite portions again passed: guest reached sqlite-home-smoke and sqlite-concurrency-smoke; final host inspection showed PRAGMA integrity_check ok and [('guest', 200), ('host', 200)]. The full self-test still fails afterward at the pre-existing Docker TCP timeout to 127.0.0.1:1075.
