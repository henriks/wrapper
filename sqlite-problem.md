# SQLite host/guest concurrency problem

## Summary

We encountered silent application-level data loss when a host process and a VM guest process concurrently wrote to the same SQLite database in a project directory shared through composed-fs/virtiofs/FUSE.

The host opened the database directly on the host filesystem. The guest opened the same pathname through the VM's virtiofs mount served by our custom Rust composed-fs backend. Both sides used Python `sqlite3` in WAL mode.

Observed failures included:

- final row counts missing committed rows from one side, e.g. `guest=200`, `host=191`;
- stronger repros with 500 writes produced cases like `guest=500`, `host=21` or only guest rows;
- `PRAGMA integrity_check` still returned `ok`;
- at least one guest run produced `sqlite3.OperationalError: disk I/O error`.

This is not just a flaky test. It is a real unsafe filesystem/coherency contract for SQLite WAL across mixed access paths.

## Environment and access pattern

The relevant architecture is:

- Linux host runs QEMU microvm.
- Project directory is mounted into the guest using virtiofs/FUSE via `agentvm-composed-fs`.
- Host can access the project directory directly through the host filesystem.
- Guest can access the same apparent files through virtiofs/composed-fs.
- The failing test used one host SQLite connection and one guest SQLite connection against the same database path.

Simplified test logic:

```python
conn = sqlite3.connect(db, timeout=30.0, isolation_level=None)
conn.execute("PRAGMA busy_timeout=30000")
mode = conn.execute("PRAGMA journal_mode=WAL").fetchone()[0].lower()
assert mode == "wal"
conn.execute("""
  CREATE TABLE IF NOT EXISTS concurrent_writes (
    source TEXT NOT NULL,
    n INTEGER NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY(source, n)
  )
""")
for i in range(200):
    with conn:
        conn.execute(
            "INSERT OR REPLACE INTO concurrent_writes(source, n, value) VALUES(?, ?, ?)",
            (source, i, f"{source}-{i}"),
        )
assert conn.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
conn.close()
```

The host worker uses the host path directly. The guest worker uses the virtiofs path.

## What we initially suspected

At first this looked like a byte-range locking bug in composed-fs. SQLite relies heavily on POSIX advisory locks, and the docs explicitly warn that broken locking can corrupt databases.

However, direct probes showed simple lock bridging works:

- guest takes a byte-range `fcntl` lock through composed-fs;
- host attempts a conflicting POSIX lock directly;
- host gets a conflict (`EAGAIN`/would-block);
- reverse-style probing should still be added, but the basic bridge is not obviously broken.

So the problem is probably not “locks do not conflict at all”.

## Actual likely cause

The likely cause is SQLite WAL's shared-memory WAL-index requirement.

SQLite WAL mode uses three files for database `X`:

- `X`: main database file;
- `X-wal`: write-ahead log containing committed frames not yet checkpointed;
- `X-shm`: shared-memory WAL-index backing file.

The important point: normal WAL correctness requires every concurrent SQLite connection to share one coherent WAL-index. SQLite implements this through `xShmMap`, `xShmLock`, `xShmBarrier`, and related VFS methods. On Unix this usually means `mmap(MAP_SHARED)` of the `X-shm` file plus byte-range locks on bytes in that file.

In our setup there are two access/coherency domains:

1. host SQLite maps and updates `X-shm` directly through the host filesystem/page cache;
2. guest SQLite maps and updates `X-shm` through guest kernel virtiofs/FUSE and the composed-fs backend.

Even if byte-range locks bridge correctly, the host-direct mmap and guest-through-virtiofs mmap are not guaranteed to be one coherent shared-memory object. Locks serialize decisions, but they do not by themselves force the guest's mmap/page-cache view of `X-shm` to observe every host update, or vice versa.

That means a process can acquire the correct lock and still read stale WAL-index fields such as:

- `mxFrame`;
- `nBackfill`;
- `read-mark[]`;
- WAL hash/page lookup tables.

A stale WAL-index can cause SQLite to make internally consistent but wrong decisions: skip committed frames, reset/reuse WAL content, checkpoint the wrong range, or fail to see another participant's committed work. The resulting DB can remain structurally valid, which explains why `PRAGMA integrity_check` returns `ok` even though application rows are missing.

## Why `integrity_check` did not catch it

`PRAGMA integrity_check` verifies SQLite's low-level B-tree/database structure. It does not know that a given application expected exactly 200 `host` rows and 200 `guest` rows.

If committed host frames are lost/skipped before becoming part of the final database state, the final database can still be a valid SQLite database. This is application-level lost-commit/data-loss behavior, not necessarily low-level page corruption.

## Experiments already tried

These did **not** fix the problem:

- pre-initializing the database, WAL mode, and table before concurrent access;
- setting `PRAGMA wal_autocheckpoint=0`;
- returning FUSE `FOPEN_DIRECT_IO` from composed-fs;
- advertising/experimenting with direct-I/O mmap support (`DIRECT_IO_ALLOW_MMAP`);
- a conservative attempt to coarsen guest-side `*-shm` locks;
- relying on ordinary byte-range lock bridging alone.

A useful datapoint:

- `PRAGMA locking_mode=EXCLUSIVE` on both host and guest avoided row loss in smoke runs.

This supports the WAL-index hypothesis. SQLite documents that EXCLUSIVE WAL can avoid `X-shm` if set before first WAL access and if the connection is effectively the only user. But this is not transparent concurrent WAL support; it is coarse serialization and requires all participants to cooperate.

## Relevant SQLite documentation interpretation

SQLite's “How To Corrupt An SQLite Database File” page says SQLite depends on correct filesystem locking and warns that broken/mismatched locking protocols can corrupt databases. It also says all connections to the same database need compatible coordination.

For our case, the more precise issue is broader than locks:

- SQLite WAL assumes both locking **and** coherent shared memory for the WAL-index.
- Host-direct access and guest virtiofs/FUSE access are not obviously the same VFS/shared-memory domain.
- A FUSE/virtiofs backend can make file operations and locks appear mostly POSIX-like while still not providing the mmap coherence SQLite WAL needs between host-direct and guest-mounted clients.

`PRAGMA mmap_size=0` is not a fix. It disables mmap of the main database file, not WAL's `X-shm` shared-memory mapping.

## Recommended contract

Do **not** support normal SQLite WAL concurrency when one participant accesses files directly on the host and another accesses the same files through composed-fs/virtiofs.

The safe default should be fail-closed, not best-effort.

Suggested contract text:

> SQLite normal WAL mode is not supported for databases in composed-fs shared project directories when any process may also access the same database files directly on the host filesystem. Normal WAL requires a coherent shared-memory WAL-index (`X-shm`) across all concurrent SQLite clients. composed-fs does not guarantee that a host-direct `mmap()` and a guest virtiofs/FUSE `mmap()` of `X-shm` are one coherent shared-memory domain. To avoid silent data loss, composed-fs may deny WAL sidecar creation/opening or fail read-write access to persisted WAL databases. Use rollback-journal mode, an SQLite service/proxy, a single-owner virtual disk, or controlled EXCLUSIVE WAL instead.

## Implementation directions

Potential implementation options, in order of preference:

1. **Fail closed for WAL sidecars in shared directories.**
   - Deny guest creation/open/write/mmap/lock operations for `*-shm`.
   - Consider also denying `*-wal` creation to prevent WAL transitions more clearly.
   - Use policy errors such as `EACCES`, `EPERM`, or `EOPNOTSUPP`, not silent fallback.
   - Emit clear logs explaining SQLite WAL is unsafe across host-direct + guest-FUSE.

2. **Detect persisted WAL databases.**
   - SQLite DB header begins with `SQLite format 3\0`.
   - WAL mode is persisted in header bytes 18 and 19, both set to `2`.
   - On guest read-write open of a persisted-WAL DB under deny policy, fail before allowing writes.

3. **Validate rollback-journal mode as the supported file-level cross-boundary mode.**
   - Use `PRAGMA journal_mode=DELETE` initially.
   - Use `PRAGMA synchronous=FULL` for validation.
   - Use `PRAGMA busy_timeout=30000`.
   - Optional: `PRAGMA mmap_size=0` to avoid main-DB mmap while validating.
   - Must stress test host+guest concurrency and crash recovery before declaring supported.

4. **Allow EXCLUSIVE WAL only as a controlled workaround.**
   - Must set `PRAGMA locking_mode=EXCLUSIVE` before `PRAGMA journal_mode=WAL` / before first WAL access.
   - Verify no `X-shm` is created.
   - All participants must comply.
   - Treat as single-client/coarse-serialized mode, not general concurrency.

5. **For real WAL concurrency, move SQLite into one coherency domain.**
   - Use a host-side or guest-side SQLite service/proxy.
   - Or use a virtual block device owned by one OS.
   - Or build/enforce a custom SQLite VFS on all participants, which is high effort.

## Tests to add

Minimum tests/probes for follow-up:

### 1. Exact SQLite lock-byte bridge test

Test guest↔host conflicts on SQLite's actual lock regions:

- main DB lock byte region:
  - `PENDING_BYTE = 0x40000000`;
  - `RESERVED_BYTE = PENDING_BYTE + 1`;
  - shared lock range begins at `PENDING_BYTE + 2`, size 510;
- WAL `X-shm` lock bytes:
  - bytes 120..127.

Test both directions: guest holds/host conflicts and host holds/guest conflicts.

### 2. `X-shm` mmap coherency probe

Create a file in the shared directory and map it on:

- host directly with `MAP_SHARED`;
- guest through virtiofs/composed-fs with `MAP_SHARED`.

Use byte-range locks for turn-taking, but do **not** use `msync()` in the primary test, because SQLite does not fsync `X-shm`.

Have each side increment/check a 32-bit counter and checksum at WAL-index-like offsets for many iterations. Any stale read, torn value, or regression confirms normal WAL is unsafe.

### 3. WAL fail-closed tests

Under the default deny policy:

- `PRAGMA journal_mode=WAL` from the guest should not return `wal` in a shared project path;
- no `X-shm` should be created;
- existing WAL DB read-write open from guest should fail before writes;
- error should be clear and documented.

### 4. Rollback-journal stress and crash tests

For `journal_mode=DELETE` + `synchronous=FULL`:

- host and guest concurrent inserts with acknowledged commit logs;
- kill host writer mid-transaction;
- kill guest writer mid-transaction;
- terminate VM mid-transaction;
- verify hot-journal recovery by the other side;
- final counts must match acknowledged commits;
- `integrity_check` must be `ok`.

### 5. EXCLUSIVE WAL test

- Set `locking_mode=EXCLUSIVE` before first WAL access.
- Verify no `X-shm` exists or is opened.
- Verify second connection blocks or gets `SQLITE_BUSY`.
- Verify data survives.

## Current ticket state

The issue is tracked as `wra-ylt9`: “Fix or re-scope flaky live-smoke host/guest SQLite concurrency check”.

The quick required `live-smoke` currently uses `--skip-sqlite-concurrency` so mandatory validation remains stable, but this should be treated as a temporary scoping change. The underlying issue is important and should be fixed by making the SQLite contract fail-safe.

Related research memo from ChatGPT Pro: `sqlite_wal_virtiofs_safety_memo.md`.

## Bottom line

The old live smoke was accidentally testing an unsafe contract: normal SQLite WAL with one process host-direct and another process guest-through-composed-fs. The right fix is not to keep retrying the test or only tweak locks. The right fix is to prevent silent WAL use across this boundary, document the unsupported mode, and add validation for the supported alternatives.
