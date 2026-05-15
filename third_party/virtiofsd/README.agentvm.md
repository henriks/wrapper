# AgentVM virtiofsd Vendor Notes

This directory is a vendored copy of the `virtiofsd` crate. It exists because
`agentvm-composed-fs` needs one API surface that `virtiofsd 1.13.3` does not
expose: full FUSE byte-range lock request details.

The crate is consumed through a path dependency:

```toml
virtiofsd = { path = "../third_party/virtiofsd", default-features = false }
```

Do not replace this dependency with the crates.io release unless upstream
exposes equivalent lock request APIs and the composed-fs lock tests still pass.

## Local Patch Surface

The intended local delta is small and should stay small:

- `src/filesystem.rs`
  - `FileSystem::getlk` receives `Context`, inode, file handle, FUSE owner,
    `fuse::FileLock`, and lock flags, and returns `fuse::FileLock` for the
    `GETLK` reply.
  - `FileSystem::setlk` receives the same request fields and returns
    `io::Result<()>`.
  - `FileSystem::setlkw` receives the same request fields and returns
    `io::Result<()>`.
- `src/server.rs`
  - `GETLK`, `SETLK`, and `SETLKW` decode `LkIn` instead of ignoring the
    request body.
  - `GETLK` replies with `LkOut`.
  - `SETLK` and `SETLKW` reply success with an empty OK response.

The wire structs already exist upstream in `src/fuse.rs`:

- `LkIn { fh, owner, lk, lk_flags, padding }`
- `LkOut { lk }`
- `FileLock { start, end, type_, pid }`

## Why This Matters

SQLite relies on byte-range locks for database and WAL coordination. If the
guest kernel is told that POSIX locks are supported, the userspace filesystem
must receive and enforce the real lock owner/range/type data. The upstream
`virtiofsd 1.13.3` trait had no-argument `getlk`, `setlk`, and `setlkw` hooks,
so `ComposedFs` could not bridge guest locks to host-visible locks safely.

`ComposedFs` now advertises `POSIX_LOCKS` and implements the bridge using Linux
OFD locks. Dropping the vendored API patch will either break compilation or,
worse, tempt a future change to advertise unsupported lock behavior.

## Updating From Upstream

Use this workflow when merging a newer `virtiofsd` release.

1. Record the current local delta:

   ```sh
   diff -ru \
     ~/.cargo/registry/src/index.crates.io-*/virtiofsd-1.13.3 \
     third_party/virtiofsd \
     > /tmp/agentvm-virtiofsd-local.patch
   ```

2. Fetch or unpack the new upstream crate outside the repo. Prefer the exact
   crates.io release source that Cargo will resolve, not a random repository
   checkout.

3. Replace `third_party/virtiofsd` with the new upstream source, preserving
   this `README.agentvm.md`.

4. Reapply the lock API patch manually. Do not blindly apply the old patch if
   upstream has changed the FUSE server or `FileSystem` trait shape.

5. Inspect whether upstream now exposes complete lock request details itself.
   If it does, prefer the upstream API and remove AgentVM-specific trait changes
   only after `ComposedFs` is ported and all lock tests pass.

6. Update `composed-fs/Cargo.lock` and `vm-frontend/Cargo.lock` through Cargo,
   then review the diff for unrelated dependency churn.

7. Run the required verification:

   ```sh
   cargo test --manifest-path composed-fs/Cargo.toml --offline
   cargo test --manifest-path composed-fs/Cargo.toml --offline \
     proptest_lock_operation_sequences_stress -- --ignored --nocapture
   cargo test --manifest-path vm-frontend/Cargo.toml --offline
   git diff --check
   ```

8. Run the live SQLite validation before treating the update as complete. The
   live self-test must reach `self-test: sqlite-concurrency-smoke`, and the
   shared database must pass `PRAGMA integrity_check` with both host and guest
   rows present.

## Review Checklist

Before merging an upstream update, confirm:

- `agentvm-composed-fs` still compiles against the path dependency.
- `ComposedFs::init` still advertises `POSIX_LOCKS`.
- `GETLK`, `SETLK`, and `SETLKW` still carry inode, handle, owner, lock range,
  lock type, and flags into `ComposedFs`.
- The lock bridge still passes deterministic tests, generated property tests,
  and the ignored stress property.
- No unrelated upstream daemon policy, sandboxing, seccomp, or passthrough
  behavior has been accidentally adopted into the embedded library path.
