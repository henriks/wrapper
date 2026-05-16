# Technical research memo: SQLite WAL safety across host-direct and guest virtiofs/FUSE access

**Date:** 2026-05-16  
**Scope:** Linux host + QEMU microvm. The host opens SQLite database files directly on the host filesystem. The guest opens the same pathname through virtiofs/FUSE using a custom Rust backend (`composed-fs`). Both sides may run ordinary Python `sqlite3` linked against the platform SQLite library.

---

## 1. Concise conclusion

**Recommended contract:** do **not** support normal-mode SQLite WAL concurrency when one participant opens the database directly on the host filesystem and another participant opens the same files through virtiofs/FUSE/composed-fs. Treat this as an unsupported, fail-closed case even if byte-range locks appear to bridge.

The observed result—`integrity_check` returns `ok`, but committed rows from one side disappear—is consistent with violating SQLite WAL's shared-memory contract for the `X-shm` WAL-index. WAL correctness is not just a byte-range lock problem. In normal WAL mode, every concurrent SQLite connection must observe one coherent shared WAL-index, plus correct lock and memory-barrier ordering. A host-direct `mmap()` of `X-shm` and a guest virtiofs/FUSE `mmap()` of `X-shm` should be assumed to be different cache/coherency domains unless proven otherwise by targeted mmap-coherency probes and sustained WAL stress tests.

`PRAGMA locking_mode=EXCLUSIVE` avoiding loss in smoke tests is strong supporting evidence: SQLite documents that EXCLUSIVE WAL can run without creating or using the shared-memory WAL-index, but only when EXCLUSIVE mode is set before the first WAL access and only when the connection is guaranteed to be the sole database user.

**Practical recommendation:**

1. Default composed-fs policy should **deny or warn/fail** SQLite WAL sidecar use in shared host/guest directories where host-direct access is possible.
2. For concurrent host+guest access, use **rollback-journal mode** only after validating locks, cache invalidation, and fsync/flush behavior.
3. For WAL performance/concurrency, route all access through one domain: a host-side or guest-side SQLite service/proxy, a virtual block device owned by one OS, or all clients using the same filesystem/coherency domain.
4. `locking_mode=EXCLUSIVE` may be offered as a controlled single-client workaround, not as transparent concurrent WAL support.

---

## 2. Exact SQLite/OS/filesystem requirements

### 2.1 SQLite WAL file set and identity

For a database named `X`, active WAL state consists of:

| File | Role | Safety requirement |
|---|---|---|
| `X` | Main database file | Same underlying inode/object and same canonical identity for all clients. Avoid hardlinks, symlinks, path aliases, and composed path translations that produce mismatched sidecars. |
| `X-wal` | Persistent write-ahead log | Must remain paired with `X`. Committed transactions may live only in `X-wal` until checkpointed. Copying/moving `X` without `X-wal` can lose committed work. |
| `X-shm` | WAL-index shared memory backing file | In normal WAL mode, all clients must share one coherent memory mapping of this file. It is coordination state and a lookup cache; it is not durable database content and is normally not fsynced. |

SQLite records WAL mode persistently in the main database header: bytes 18 and 19 are both `2` in WAL mode. A fail-closed backend can use this as a detector for already-WAL databases.

### 2.2 POSIX byte-range locks

The lock bridge must support both main database locks and WAL-index locks. The user's existing probe demonstrates a simple guest-to-host POSIX byte-range conflict, but the implementation should verify SQLite's exact lock bytes as well.

**Main database lock-byte page** in the default SQLite Unix locking scheme:

| Lock region | Default offset |
|---|---:|
| `PENDING_BYTE` | `0x40000000` |
| `RESERVED_BYTE` | `PENDING_BYTE + 1` |
| `SHARED_FIRST` | `PENDING_BYTE + 2` |
| `SHARED_SIZE` | `510` bytes |

These values come from SQLite's OS abstraction source. If SQLite is compiled with a changed `PENDING_BYTE`, the database format/locking protocol is subtly incompatible with the default. In normal deployments, assume the default values above.

**WAL-index lock bytes** are in the first 136 bytes of `X-shm`:

| WAL lock | `xShmLock` offset | File byte in `X-shm` |
|---|---:|---:|
| `WAL_WRITE_LOCK` | 0 | 120 |
| `WAL_CKPT_LOCK` | 1 | 121 |
| `WAL_RECOVER_LOCK` | 2 | 122 |
| `WAL_READ_LOCK(0)` | 3 | 123 |
| `WAL_READ_LOCK(1)` | 4 | 124 |
| `WAL_READ_LOCK(2)` | 5 | 125 |
| `WAL_READ_LOCK(3)` | 6 | 126 |
| `WAL_READ_LOCK(4)` | 7 | 127 |

Required behavior:

- Locks taken by the guest through virtiofs/FUSE must conflict with locks taken by host-direct processes on the same underlying file.
- Lock acquisition/release must not be reordered with protected WAL-index updates in a way that lets a peer acquire a lock but see stale `X-shm` fields.
- The lock namespace must be per underlying file, not per guest path, per file handle, or per FUSE node cache entry.
- QEMU/virtiofsd-style deployments must enable remote POSIX locks. QEMU's historical `virtiofsd` option defaults are not safe to assume: its docs list `posix_lock|no_posix_lock` with default `no_posix_lock` for that version.

### 2.3 WAL-index shared-memory coherence

Normal SQLite WAL requires all concurrent clients to share the same WAL-index. The WAL-index contains, among other fields:

| Field | Offset in `X-shm` | Meaning |
|---|---:|---|
| `iChange` | 8..11 | Incremented with transactions. |
| `isInit` | 12 | Header initialized flag. |
| `mxFrame` | 16..19 and copy at 64..67 | Last valid committed frame in the WAL. |
| `nBackfill` | 96..99 | Number of WAL frames checkpointed into the database. |
| `read-mark[0..4]` | 100..119 | Reader marks protected by corresponding read locks. |
| lock bytes | 120..127 | Bytes used by `xShmLock`; SQLite does not read or write these bytes as data. |
| hash/page arrays | after 136 | Native-order WAL lookup tables. |

The backend/filesystem must provide:

- Coherent `MAP_SHARED` visibility for `X-shm` between host-direct and guest-through-FUSE clients without requiring `msync()` or `fsync()` of `X-shm`. SQLite says `X-shm` is never fsynced.
- Atomic-enough aligned 32-bit load/store behavior for WAL-index words on all participating architectures.
- Correct memory-barrier semantics for SQLite's `xShmBarrier`: data written before a lock release or barrier must be visible after a peer acquires the corresponding lock or executes the barrier path.
- Native byte order compatibility. In this scenario, host and guest are presumably same architecture/endian; if not, normal WAL shared-memory is not portable because `X-shm` numeric fields are native order.

A byte-range lock bridge alone does not establish mmap cache coherency. Locks serialize decisions; they do not necessarily invalidate or propagate dirty cachelines in another kernel's page cache.

### 2.4 WAL and database file I/O ordering

SQLite relies on `xSync`/`fsync`/`fdatasync` as durability operations and, in some modes, as I/O barriers:

- WAL commits append frames to `X-wal` and may sync `X-wal` depending on `PRAGMA synchronous`.
- Checkpointing must sync `X-wal` before moving frames into `X`, and sync `X` before resetting/reusing the WAL.
- Rollback-journal mode relies more heavily on journal creation/deletion/truncation, directory visibility, and database-file sync ordering.
- `PRAGMA synchronous=FULL` is the conservative setting for validation. `NORMAL` can be acceptable for some durability tradeoffs, but it should not be used to prove filesystem correctness.

### 2.5 Regular SQLite mmap is separate from WAL shared memory

`PRAGMA mmap_size=0` disables memory-mapped I/O for bytes of the **main database file**. It does not disable SQLite WAL's `X-shm` mapping. Therefore it is not a fix for this failure mode. It can still be useful as a defensive setting while validating rollback mode or avoiding unrelated database-file mmap issues.

### 2.6 Version and compile-time requirements

Capture these for every host and guest test run:

```bash
python3 - <<'PY'
import sqlite3, sys
con = sqlite3.connect(':memory:')
print('python', sys.version)
print('sqlite3 module version', sqlite3.version)
print('SQLite runtime', sqlite3.sqlite_version)
print('compile options')
for row in con.execute('PRAGMA compile_options'):
    print(' ', row[0])
PY
```

Important dependencies:

- Use a SQLite version that includes the 2026 WAL-reset bug fix: SQLite 3.51.3 or later, or the documented backports 3.44.6 / 3.50.7. The bug is described as rare, and it does not explain this repro by itself, but it is a confounder for WAL stress tests.
- `SQLITE_OMIT_WAL` must not be set if testing WAL.
- `SQLITE_DEFAULT_LOCKING_MODE`, `SQLITE_DEFAULT_MMAP_SIZE`, `SQLITE_MAX_MMAP_SIZE`, `SQLITE_ENABLE_8_3_NAMES`, and VFS selection can change observed behavior.
- `FUSE_DIRECT_IO_ALLOW_MMAP` depends on a sufficiently recent guest kernel/FUSE protocol negotiation. Kernel changelog evidence places the renamed `FUSE_DIRECT_IO_ALLOW_MMAP` flag in the Linux 6.6 series; verify by negotiation/logging, not assumption.
- virtiofs DAX/coherence behavior depends on guest kernel, QEMU, virtiofsd or custom backend, mount options, and whether `STATX_ATTR_DAX` is actually set for the file.

---

## 3. Likely root cause of the observed data loss

The most likely root cause is **incoherent WAL-index shared memory (`X-shm`) across host-direct and guest virtiofs/FUSE access paths**.

Mechanism:

1. Host SQLite maps and updates `X-shm` through the host kernel and the underlying host filesystem/page cache.
2. Guest SQLite maps and updates the same apparent `X-shm` path through the guest kernel's virtiofs/FUSE client and composed-fs backend.
3. POSIX byte-range locks may bridge correctly, but lock correctness does not imply that each side's `mmap()` view of `X-shm` is a single coherent shared-memory object.
4. A writer/checkpointer/reader may acquire the right lock but read stale `mxFrame`, `nBackfill`, `read-mark[]`, or hash-table content.
5. SQLite's WAL logic can then make a valid but wrong decision: reuse/reset a WAL segment, skip frames during checkpoint, fail to see the other side's committed frames, or read a stale snapshot.
6. The final database can remain structurally valid; therefore `PRAGMA integrity_check` can return `ok` while application-level rows are missing.

This explains the probes:

- **Byte-range lock bridge works:** necessary but insufficient.
- **Pre-initializing WAL/table does not fix:** initialization is not the only shared-memory update; `mxFrame`, `nBackfill`, read marks, and hashes change continuously.
- **`wal_autocheckpoint=0` does not fix:** all WAL reads/writes still depend on `X-shm`; WAL reset can also occur as part of writer behavior when WAL content has been backfilled and no readers use it.
- **`FOPEN_DIRECT_IO` does not fix:** direct I/O changes read/write page-cache behavior; SQLite's WAL-index is `mmap()`-based shared memory. With direct I/O, shared mmap is disabled by default unless `FUSE_DIRECT_IO_ALLOW_MMAP` is negotiated, and allowing mmap does not prove cross-domain coherence with host-direct mappings.
- **Coarsening guest `X-shm` locks does not fix:** the stale data hazard remains.
- **`locking_mode=EXCLUSIVE` avoids loss:** this mode can omit `X-shm` entirely if set before first WAL access, so it removes the suspected incoherent shared-memory object.
- **`sqlite3.OperationalError: disk I/O error`:** plausible if the backend returns EIO/EOPNOTSUPP/EACCES on a WAL `xShmMap`, lock, mmap, truncate, or sidecar operation. It is also consistent with direct-I/O/mmap combinations that SQLite's Unix VFS did not expect to be safe.

A secondary possibility is SQLite's documented WAL-reset bug in versions before 3.51.3 or the listed backports. However, that bug is described as rare and timing-specific; it does not fit frequent large row loss that correlates with a host-direct/guest-FUSE boundary and disappears under EXCLUSIVE mode. Still, eliminate it before final certification.

---

## 4. Answers to the research questions

### 4.1 What exact guarantees does SQLite WAL require?

SQLite WAL normal mode requires all of the following:

- Same database identity and sidecar identity for `X`, `X-wal`, and `X-shm`.
- Correct main-database locks and WAL `xShmLock` byte locks.
- One coherent shared-memory WAL-index mapping among all concurrent clients.
- Memory-barrier semantics sufficient for `xShmBarrier` and lock handoff.
- Correct visibility of WAL-index updates after lock acquire/release.
- Correct WAL append, file-size, truncate, checkpoint, and database-file write visibility.
- Correct `fsync`/`fdatasync`/flush semantics, at least as an I/O barrier.
- No direct rogue writes to the database, WAL, journal, or sidecars.
- No hardlink/symlink/path alias causing clients to use different sidecar names.

### 4.2 Is host-direct + guest-through-virtiofs/FUSE known to violate those guarantees?

I did not find an authoritative SQLite, kernel, or virtiofs document that explicitly says: “SQLite WAL is unsafe with one host-direct process and one virtiofs guest process.” The safer conclusion is contractual: **the combination is unsafe unless you prove that `X-shm` is one coherent shared-memory mapping across those two access paths**.

The documented pieces point in that direction:

- SQLite WAL requires shared memory and says normal WAL is unsuitable where processes cannot share memory.
- Kernel FUSE cached and writeback modes introduce a FUSE-client page-cache domain; writeback mode explicitly assumes all changes go through the FUSE kernel module. Host-direct accesses do not go through the guest FUSE kernel module.
- virtiofs is based on FUSE. It aims for local-file semantics, but its own design distinguishes local single-mount coherency from remote/weaker coherency and treats DAX/shared mapping as an implementation-specific facility.
- QEMU virtiofsd cache options trade coherence for performance; `cache=none` improves ordinary file coherency but does not by itself prove WAL `mmap()` shared-memory semantics.

### 4.3 Can a FUSE/virtiofs backend tell SQLite “do not use WAL shared memory”?

Not cleanly from the filesystem layer when the application uses the built-in SQLite Unix VFS through Python `sqlite3`.

SQLite decides whether WAL shared memory exists through its VFS `sqlite3_io_methods` methods: `xShmMap`, `xShmLock`, `xShmBarrier`, and `xShmUnmap`. A custom C VFS can omit or replace those methods. A FUSE filesystem is below the built-in Unix VFS; SQLite still sees a normal Unix filesystem that supports `open`, `mmap`, `fcntl`, and file creation unless the backend forces errors.

Possible outcomes:

- If the VFS itself lacks shared-memory methods, `PRAGMA journal_mode=WAL` returns the old mode instead of `wal`, or opening an already-WAL database fails.
- If EXCLUSIVE locking mode is set before first WAL access, SQLite never calls the shared-memory methods and no `X-shm` is created.
- If composed-fs denies `X-shm` creation/open/mmap/locking, SQLite should fail WAL conversion/open rather than silently falling back to safe multi-process WAL. This is a useful fail-closed policy, but you must test exact SQLite error behavior.
- `PRAGMA mmap_size=0` does not disable `X-shm`.

### 4.4 Would disabling mmap for regular file contents help?

No for the observed WAL loss. It may prevent unrelated database-file mmap issues, but the suspected hazard is the WAL-index `X-shm`, which is separate from normal database-file mmap controlled by `PRAGMA mmap_size`.

### 4.5 Does `FOPEN_DIRECT_IO` or FUSE direct I/O affect WAL `X-shm` mmap semantics?

It affects ordinary read/write caching and can affect whether shared mmap is permitted for direct-I/O files, but it is not a SQLite WAL safety guarantee.

- In FUSE direct-io mode, the page cache is bypassed for reads/writes.
- Shared mmap is disabled by default in direct-io mode.
- `FUSE_DIRECT_IO_ALLOW_MMAP` can allow shared mmap while still bypassing cache for regular reads/writes.

This matches the observed experiment: direct I/O can produce SQLite I/O errors if WAL shared mmap cannot be established, and allowing mmap can make SQLite run without proving host-direct/guest-FUSE coherency.

### 4.6 Is rollback-journal mode safe across the boundary if locks bridge correctly?

Rollback mode is the most plausible file-based option because it avoids the WAL `X-shm` shared-memory requirement. It can be safe only if these are true:

- SQLite main database locks bridge correctly across host and guest.
- Regular file read/write visibility is coherent after lock handoff.
- Journal file create/delete/truncate/rename and directory metadata are visible to both sides at the right times.
- `fsync`/`fdatasync`/flush calls are forwarded and act as required barriers.
- FUSE writeback caching is disabled or otherwise proven safe even though host-direct changes bypass the guest FUSE kernel module.
- No process deletes hot journals or copies/moves the DB without its journal.

Use rollback mode as a tested contract, not as an assumption.

### 4.7 Is `locking_mode=EXCLUSIVE` a valid workaround?

Yes, but only as a controlled serialization policy:

- Set `PRAGMA locking_mode=EXCLUSIVE` before `PRAGMA journal_mode=WAL` or before the first access to an already-WAL database.
- Every connection that might touch the database must comply.
- Verify that no `X-shm` file is created or mapped.
- The second concurrent connection should block or return `SQLITE_BUSY` until the exclusive connection closes.
- If any normal-mode WAL connection opens first, SQLite creates the shared-memory WAL-index; later switching to EXCLUSIVE is not the same no-shm guarantee.

This workaround relies on the database-file exclusive lock bridging correctly. It is operationally fragile if arbitrary host or guest tools can open the DB without the required pragma.

### 4.8 Can composed-fs enforce safety?

Yes, by making unsafe modes fail closed. Recommended enforcement options:

1. **Deny `X-shm` creation/open/mmap/locking** for guest paths in shared host-direct directories unless an explicit experimental policy is enabled. Denying `X-shm` causes normal WAL to fail rather than silently using incoherent shared memory.
2. **Optionally deny `X-wal` creation** to prevent new WAL transitions. Denying only `X-shm` is the key WAL-index safety measure; denying `X-wal` provides clearer policy enforcement for new WAL attempts.
3. **Detect persisted WAL mode** by reading database header bytes 18 and 19. If both are `2`, block guest read-write open under a “no cross-boundary WAL” policy until the DB is converted back to rollback mode by a safe process.
4. **Detect suffixes** `-shm`, `-wal`, `.SHM`, `.WAL` because SQLite can use 8.3-compatible sidecar naming with `SQLITE_ENABLE_8_3_NAMES`.
5. **Emit explicit diagnostics**: path, PID/VM, attempted sidecar, and recommended remediation (`PRAGMA journal_mode=DELETE`, use DB service, or exclusive WAL in controlled mode).
6. **Do not rely on direct I/O or cache invalidation alone** for WAL normal mode.
7. **Do not try to fix by coarsening locks alone**; mmap coherency remains the hard requirement.

Potential error choices: prefer `EACCES`, `EPERM`, or `EOPNOTSUPP` for policy denial rather than `EIO`, because `EIO` suggests media failure and can obscure the policy cause. Validate SQLite's exact mapped error (`SQLITE_CANTOPEN`, `SQLITE_READONLY`, or `SQLITE_IOERR`) in CI.

### 4.9 If the robust answer is “do not support this,” what should the documented contract be?

Document:

> Project directories shared by composed-fs are not a supported SQLite normal-WAL coherency domain when any process can also access the same database files directly on the host. Concurrent SQLite access across host-direct and guest-virtiofs paths must use rollback-journal mode, a single-domain SQLite service/proxy, exclusive WAL under strict policy, or a virtual block device owned by one side. composed-fs may reject WAL sidecar creation/opening to prevent silent data loss.

### 4.10 What validation tests should ensure fail-safe behavior?

See the detailed test plan in section 7. The minimum CI gate should include:

- WAL sidecar-denial test: `PRAGMA journal_mode=WAL` must not return `wal` under deny policy.
- Existing-WAL test: guest read-write open of a WAL-mode DB must fail before writes.
- Rollback stress test: host+guest concurrent inserts with no `X-shm`, high iteration count, crash recovery.
- Exclusive-WAL test: no `X-shm` creation and second connection blocks/busies.
- Mmap coherency probe: host-direct `MAP_SHARED` vs guest virtiofs `MAP_SHARED` on the same file, no `msync`, lock handoff, millions of iterations.

---

## 5. Recommended implementation options

| Option | Recommendation | Why | Tests/probes to confirm or falsify |
|---|---|---|---|
| A. **Fail closed for normal WAL across host-direct + guest-FUSE** | **Default** | Directly addresses suspected `X-shm` hazard. Prevents silent committed-row loss. | Deny `X-shm`; `PRAGMA journal_mode=WAL` returns non-`wal` or raises; no rows can be written in WAL mode; existing WAL DB fails before writes. |
| B. **Rollback-journal mode for cross-boundary file concurrency** | Supported after validation | Avoids `X-shm` shared memory. Still needs locks, cache visibility, fsync, and journal sidecar correctness. | High-count host+guest stress in `DELETE`, `TRUNCATE`, and/or `PERSIST`; no `X-shm`; crash/kill tests; verify final counts and hot-journal recovery. |
| C. **SQLite service/proxy** | Best for WAL concurrency | All SQLite access occurs in one process or one coherency domain. Avoids cross-domain VFS mismatch. | Concurrency stress against service API; verify no host/guest direct file opens; service handles busy/retry and checkpointing. |
| D. **Virtual block device owned by one side** | Good if guest-owned DB is acceptable | SQLite sees a local filesystem in one OS. Host must not mount/write same FS concurrently. | Standard SQLite WAL stress inside owner OS; host only accesses via service, shutdown, or read-only snapshot. |
| E. **EXCLUSIVE WAL** | Controlled workaround | SQLite can omit `X-shm`; serializes access using exclusive DB lock. | Set EXCLUSIVE before WAL access; verify no `X-shm` open/mmap; second process gets `SQLITE_BUSY`; noncompliant connection is blocked by policy. |
| F. **virtiofs DAX / special coherent mmap path** | Experimental only | Could make shared mmap coherent in some implementations, but must be proven and version-pinned. | `STATX_ATTR_DAX` verification; mmap counter probe; WAL stress matrix; kernel/QEMU/backend version pinning. |
| G. **Special SQLite VFS on both sides** | High-effort future | A VFS can implement `xShm*` correctly or fail WAL safely. But host-direct Python SQLite will not use it unless explicitly configured. | VFS unit tests for `xShmMap`, `xShmLock`, `xShmBarrier`; run SQLite TH3/sqllogictest or equivalent; enforce both host and guest use the VFS. |

---

## 6. Implementation plan

### 6.1 Policy and detection

Add a composed-fs SQLite policy mode:

```text
sqlite_wal_policy = deny | allow_exclusive_only | allow_experimental
```

Default: `deny` for project directories where host-direct access is possible.

In `deny` mode:

- Reject guest `LOOKUP`, `CREATE`, `OPEN`, `MKNOD`, `READWRITE`, `MMAP`, and lock operations for sidecar paths matching:
  - `*-shm`
  - `*-wal` if denying WAL creation entirely
  - `*.SHM` and `*.WAL` for 8.3 naming builds
- On opening a candidate SQLite DB read-write, optionally read bytes 0..100:
  - Header starts with `SQLite format 3\0`.
  - Bytes 18 and 19 both `2` => persisted WAL mode.
  - If WAL mode is detected and policy denies WAL, fail read-write open with a clear policy error.
- If sidecars already exist, warn or fail before allowing writes.
- Emit structured logs:
  - database path
  - sidecar path
  - guest PID/UID if available
  - operation denied
  - remediation hint

Use policy errors, not silent mode changes. SQLite applications often assert `journal_mode=WAL`; fail fast is preferable.

### 6.2 Safe rollback-journal profile

For applications that need host+guest file-level concurrency:

```sql
PRAGMA journal_mode=DELETE;       -- or TRUNCATE/PERSIST after tests
PRAGMA synchronous=FULL;
PRAGMA busy_timeout=30000;
PRAGMA mmap_size=0;               -- optional: disables main DB mmap only
```

Backend/mount constraints:

- Enable POSIX byte-range locks and verify exact SQLite lock byte conflicts.
- Disable FUSE writeback caching for SQLite paths, or prove it safe. If using QEMU virtiofsd-like options, prefer `cache=none`, `no_writeback`, and `posix_lock` equivalents.
- Ensure create/delete/truncate metadata invalidation is immediate enough for hot-journal detection.
- Forward `fsync`/`fdatasync`/flush and directory syncs correctly.

### 6.3 Controlled EXCLUSIVE WAL profile

For a controlled single-process/single-connection mode:

```sql
PRAGMA locking_mode=EXCLUSIVE;
PRAGMA journal_mode=WAL;
PRAGMA busy_timeout=30000;
```

Requirements:

- Must run before table creation or any first WAL access.
- Keep the exclusive connection open for the operation.
- All participants must use this profile; noncompliant normal-WAL opens must be blocked or treated as unsupported.
- CI must prove no `X-shm` is created or mmaped.

### 6.4 WAL concurrency profile

For real WAL concurrency, do not mix host-direct and guest-FUSE paths. Choose one:

- Host-side SQLite service with RPC/IPC from the guest.
- Guest-side SQLite service with host access via RPC/IPC.
- A virtual disk mounted by one OS; other side accesses through service/snapshot only.
- A custom SQLite VFS deployed and enforced on both host and guest.

---

## 7. Test plan

### 7.1 Environment capture test

Run on host and guest for every CI job and attach to logs:

```bash
python3 - <<'PY'
import sqlite3, sys, os, platform
con = sqlite3.connect(':memory:')
print('platform', platform.platform())
print('python', sys.version)
print('sqlite3 module', sqlite3.version)
print('SQLite runtime', sqlite3.sqlite_version)
print('pid', os.getpid())
print('compile_options')
for (opt,) in con.execute('PRAGMA compile_options'):
    print(opt)
PY
uname -a
mount | grep -E 'virtiofs|fuse' || true
```

Pass criteria:

- SQLite runtime is known and patched for the WAL-reset bug, or the test is explicitly marked “pre-fix exploratory.”
- FUSE/virtiofs flags and cache settings are visible in logs.

### 7.2 Exact SQLite lock bridge test

Extend the existing byte-range lock probe to these exact offsets:

- Main DB: `0x40000000`, `0x40000001`, `0x40000002..0x40000201`.
- `X-shm`: bytes 120..127.

Procedure:

1. Guest opens through composed-fs and takes an exclusive POSIX lock on each byte/range.
2. Host direct process attempts conflicting lock on underlying file.
3. Reverse host and guest roles.
4. Inspect `/proc/locks` and `/proc/<pid>/fdinfo/*` if available.

Pass criteria:

- Conflicting locks fail with `EAGAIN`/`EACCES` or block as expected.
- Locks are on the same underlying inode/object.
- Lock release ordering is deterministic.

Fail implication:

- Neither rollback nor EXCLUSIVE WAL is safe until locks are fixed.

### 7.3 `X-shm` mmap coherence microprobe

Purpose: directly test the suspected root cause outside SQLite.

File: create `probe-shm` in the same shared directory, size 32768 bytes.

Protocol:

1. Host maps `probe-shm` with `MAP_SHARED` through the direct host path.
2. Guest maps the same file with `MAP_SHARED` through virtiofs/composed-fs.
3. Use POSIX byte lock 120 as a turn lock.
4. Host increments a 32-bit counter at offset 16, writes a checksum at offset 20, releases lock.
5. Guest acquires lock, reads counter/checksum, verifies monotonicity, increments, releases.
6. Repeat 1,000,000+ iterations in both directions.
7. Do **not** call `msync()` or `fsync()` in the main test; SQLite does not fsync `X-shm`.
8. Repeat with `msync(MS_SYNC|MS_INVALIDATE)` variants to see whether explicit invalidation masks the issue.
9. Repeat at offsets 0, 8, 16, 96, 100..119, 120..127, 136, and in the hash-table area.

Pass criteria:

- No stale reads, regressions, torn 32-bit values, or checksum mismatches without `msync()`.

Fail implication:

- Normal WAL across this boundary is definitively unsafe.

Passing implication:

- Necessary but not sufficient. Still run full SQLite WAL stress because WAL uses more complex lock/readmark/checkpoint interactions.

### 7.4 WAL stress matrix

Use unique primary keys and an out-of-band commit-ack log so the test can distinguish uncommitted work from lost committed work.

Suggested matrix:

| Case | Host path | Guest path | Journal/locking | Expected result |
|---|---|---|---|---|
| H1 | host direct | host direct | WAL NORMAL | Pass; baseline. |
| G1 | guest FUSE | guest FUSE | WAL NORMAL | Pass if one guest coherency domain is valid. |
| X1 | host direct | guest FUSE | WAL NORMAL | Expected unsupported; should fail under deny policy or reproduce loss under experimental policy. |
| X2 | host direct | guest FUSE | WAL + `mmap_size=0` | If root is `X-shm`, still unsafe. |
| X3 | host direct | guest FUSE | WAL + `wal_autocheckpoint=0` | If root is `X-shm`, still unsafe. |
| X4 | host direct | guest FUSE | WAL + EXCLUSIVE before first access | Should serialize; no loss; no `X-shm`. |
| R1 | host direct | guest FUSE | rollback DELETE + FULL | Candidate supported; must pass stress and crash tests. |
| R2 | host direct | guest FUSE | rollback TRUNCATE/PERSIST + FULL | Optional supported variants after tests. |

Pass criteria for supported modes:

- Final row counts exactly match acknowledged commits.
- `PRAGMA integrity_check` returns `ok`.
- No `X-shm` exists in rollback or EXCLUSIVE no-shm WAL cases.
- Repeated runs under load, random sleeps, and process interleavings pass.

### 7.5 Fail-closed sidecar tests

Under composed-fs `sqlite_wal_policy=deny`:

1. New rollback DB: guest runs `PRAGMA journal_mode=WAL`.
   - Expected: return value is not `wal`, or Python raises a clear SQLite error.
   - Verify DB header bytes 18/19 remain rollback-mode values.
   - Verify no `X-shm` is created.
2. Existing WAL DB with sidecars present: guest attempts read-write open and insert.
   - Expected: fail before any write.
3. Existing WAL DB with `X-wal` present but `X-shm` absent.
   - Expected: fail before write; host can still recover/checkpoint directly.
4. EXCLUSIVE-only policy: guest runs EXCLUSIVE WAL sequence.
   - Expected: succeeds only if no `X-shm` is opened/created.
   - Non-EXCLUSIVE WAL sequence must fail.

### 7.6 Rollback-journal crash tests

Run with `journal_mode=DELETE` and `synchronous=FULL` first.

Scenarios:

- Kill host writer mid-transaction (`SIGKILL`).
- Kill guest writer mid-transaction.
- Power off the VM while guest writer is active.
- Kill/restart composed-fs while guest writer is active.
- Crash after journal creation but before commit; verify hot journal recovery by the other side.
- Crash after commit; verify committed rows persist.

Pass criteria:

- No missing acknowledged commits.
- Transactions killed before commit may roll back.
- Hot journals are recovered automatically.
- `integrity_check` returns `ok`.
- No stale journal deletion by backend cleanup.

### 7.7 EXCLUSIVE WAL tests

Procedure:

1. Remove existing sidecars after a clean checkpoint.
2. Host opens direct and runs:

```sql
PRAGMA locking_mode=EXCLUSIVE;
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS t(x PRIMARY KEY, y);
```

3. Guest attempts to open and read/write.
4. Reverse host and guest roles.
5. Trace filesystem operations (`strace`, backend logs, or FUSE debug).

Pass criteria:

- No `X-shm` creation/open/mmap.
- Second connection blocks or returns `database is locked` / `SQLITE_BUSY`.
- Counts match acknowledged commits.
- If any normal-mode connection opens first and creates `X-shm`, the test fails the EXCLUSIVE-only contract.

### 7.8 DAX / direct-I/O experimental tests

Only for an experimental allowlist:

- Verify `STATX_ATTR_DAX` from inside the guest for `X`, `X-wal`, and `X-shm`.
- Log negotiated FUSE flags including `FUSE_DIRECT_IO_ALLOW_MMAP` and DAX flags.
- Run the mmap counter probe without `msync()`.
- Run WAL stress with automatic and manual checkpoints.
- Pin exact versions: guest kernel, host kernel, QEMU, virtiofsd/composed-fs, SQLite runtime.

Pass criteria:

- Same as normal WAL stress, across long-duration and load tests.

Policy implication:

- Even if it passes, document as version-pinned experimental, not as the general composed-fs contract.

### 7.9 Fsync/flush/barrier probes

For rollback and any experimental WAL support:

- Instrument composed-fs to log `fsync`, `fdatasync`, `flush`, `release`, `setattr/truncate`, `unlink`, and directory sync equivalents.
- Use `strace` on host-direct SQLite and guest-side SQLite where possible.
- Confirm SQLite sync calls are forwarded and completed before composed-fs replies success.
- Run crash tests with sync points.

Fail implication:

- Rollback mode is not safe until flush/barrier behavior is fixed.

---

## 8. Risks and unknowns

- **SQLite version confounder:** SQLite documents a WAL-reset bug present through 3.51.2 and fixed in 3.51.3/backports. Upgrade or pin patched SQLite before final certification.
- **FUSE/virtiofs version variance:** direct I/O, shared mmap in direct I/O, DAX, inode invalidation, and cache options vary by kernel and backend. Probe negotiated behavior at runtime.
- **DAX is not a blanket answer:** virtiofs DAX can improve mmap coherence in some configurations, but it must be verified per file and per version. It is not a substitute for SQLite WAL tests.
- **Host-direct bypasses guest FUSE caching:** writeback-cache mode is especially suspect because it assumes all changes go through the FUSE kernel module.
- **`integrity_check` is not a lost-commit detector:** it verifies low-level structure, not that every acknowledged application transaction survived.
- **Sidecar detection is heuristic:** suffix detection can false-positive; DB-header detection can miss future transitions after open. Use both and test.
- **POSIX lock quirks:** a non-SQLite open/read/close in the same host process can drop POSIX advisory locks on Unix. Avoid direct DB file inspection by processes with live SQLite connections.
- **Path aliases:** hardlinks, symlinks, composed path aliases, and bind mounts can split sidecar names or lock identities.
- **Security/sandboxing:** rejecting sidecars should not leak host paths or allow guest policy bypass through alternate names.

---

## 9. References with quotes and links

The quotes below are intentionally short; the surrounding analysis in this memo is paraphrased from the linked primary documentation.

| Topic | Link | Short quote | Relevance |
|---|---|---|---|
| SQLite WAL overview | https://sqlite.org/wal.html | “network filesystem” | SQLite explicitly ties WAL to shared memory among clients. |
| SQLite WAL-index implementation | https://sqlite.org/wal.html#implementation_of_shared_memory_for_the_wal_index | “ordinary file” | `X-shm` is the default WAL-index shared-memory mechanism. |
| SQLite WAL without shared memory | https://sqlite.org/wal.html#use_of_wal_without_shared_memory | “EXCLUSIVE before first access” | Explains why EXCLUSIVE WAL avoids `X-shm` if set early. |
| SQLite WAL file format | https://sqlite.org/walformat.html | “not actually used as a file” | `X-shm` is a memory-mapped coordination object, not durable content. |
| SQLite WAL lock bytes | https://sqlite.org/walformat.html#wal_locks | “Eight bytes of space are set aside” | Defines `X-shm` lock bytes 120..127. |
| SQLite WAL reset | https://sqlite.org/walformat.html#reset_the_wal_file | “rewind the WAL” | Stale `mxFrame`/`nBackfill`/readmark visibility can affect reset decisions. |
| SQLite VFS `xShm*` | https://sqlite.org/c3ref/io_methods.html | “xShmMap” | WAL shared memory is a VFS-level contract, not merely a filesystem file. |
| SQLite locking mode pragma | https://sqlite.org/pragma.html#pragma_locking_mode | “without the use of shared memory” | Documents EXCLUSIVE WAL behavior. |
| SQLite mmap pragma | https://sqlite.org/pragma.html#pragma_mmap_size | “database file” | `mmap_size` is for main DB file mmap, not WAL `X-shm`. |
| SQLite rollback locking | https://sqlite.org/lockingv3.html | “Only one EXCLUSIVE lock is allowed” | Rollback mode relies on main database locks and rollback journals. |
| SQLite corruption causes | https://sqlite.org/howtocorrupt.html | “SQLite uses file locks” | Confirms lock and sync assumptions are delegated to OS/filesystem. |
| SQLite sync requirement | https://sqlite.org/howtocorrupt.html#failure_to_sync | “sync operation can be thought of as an I/O barrier” | Fsync/flush ordering matters even if durability is relaxed. |
| SQLite WAL-reset bug | https://sqlite.org/wal.html#the_wal_reset_bug | “fixed in version 3.51.3” | Version dependency to eliminate during tests. |
| Linux FUSE I/O modes | https://docs.kernel.org/filesystems/fuse/fuse-io.html | “Shared mmap is disabled by default.” | Direct I/O is not a transparent WAL shm fix. |
| Linux FUSE writeback cache | https://docs.kernel.org/filesystems/fuse/fuse-io.html | “all changes to the filesystem go through the FUSE kernel module” | Host-direct writes bypass this assumption. |
| Linux virtiofs docs | https://docs.kernel.org/filesystems/virtiofs.html | “guest acts as the FUSE client” | virtiofs uses FUSE protocol across guest/host. |
| virtiofs design | https://virtio-fs.gitlab.io/design.html | “all accesses go through a single mount” | Local coherency is contrasted with shared/remote models. |
| virtiofs DAX overview | https://virtio-fs.gitlab.io/ | “coherent between virtual machines” | DAX may be relevant but is experimental and must be proven for host-direct + guest. |
| Linux DAX docs | https://docs.kernel.org/filesystems/dax.html | “DAX removes the copy” | DAX changes page-cache/mmap behavior; version/config dependent. |
| QEMU virtiofsd options | https://qemu.readthedocs.io/en/v7.2.19/tools/virtiofsd.html | “default is `no_posix_lock`” | POSIX locks must be explicitly verified/enabled in virtiofs-like deployments. |
| QEMU virtiofsd cache options | https://qemu.readthedocs.io/en/v7.2.19/tools/virtiofsd.html | “none forbids the FUSE client from caching” | Cache mode affects ordinary file coherency; not a WAL shm guarantee. |
| FUSE direct mmap flag | https://github.com/libfuse/libfuse/blob/master/include/fuse_kernel.h | “allow shared mmap in FOPEN_DIRECT_IO mode” | Explains `FUSE_DIRECT_IO_ALLOW_MMAP` capability. |
| Linux 6.6.8 changelog | https://www.kernel.org/pub/linux/kernel/v6.x/ChangeLog-6.6.8 | “allow shared mmap of DIRECT_IO files” | Version clue for the renamed direct-I/O mmap flag. |
| SQLite lock-byte constants | https://github.com/sqlite/sqlite/blob/master/src/os.h | “first byte past the 1GB boundary” | Defines default main database lock-byte region. |

---

## 10. Bottom-line contract text for composed-fs documentation

> SQLite normal WAL mode is not supported for databases in composed-fs shared project directories when any process may also access the same database files directly on the host filesystem. Normal WAL requires a coherent shared-memory WAL-index (`X-shm`) across all concurrent SQLite clients. composed-fs does not guarantee that a host-direct `mmap()` and a guest virtiofs/FUSE `mmap()` of `X-shm` are one coherent shared-memory domain. To avoid silent data loss, composed-fs may deny WAL sidecar creation/opening or fail read-write access to persisted WAL databases. Use rollback-journal mode, an SQLite service/proxy, a single-owner virtual disk, or controlled EXCLUSIVE WAL instead.
