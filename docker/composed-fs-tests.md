# ComposedFs Correctness And Adversarial Tests

Ticket: `wra-udix`

Date: 2026-05-13

## Command

Run the backend unit tests with:

```sh
cargo test --manifest-path composed-fs/Cargo.toml --offline
```

The build check used while implementing these tests is:

```sh
cargo build --manifest-path composed-fs/Cargo.toml --offline
```

## Coverage Added

The current test suite covers:

- synthetic lookup and readdir
- directory mounts
- file mounts
- nested mount boundaries
- hardlink inode reuse by `(mount, dev, ino)`
- lookup count decrement through `forget`
- unsafe component rejection
- symlink escape attempts
- readonly rejection for create, write-open, access, setattr, mkdir, unlink,
  rmdir, rename, link, symlink, and mknod
- cross-mount rename/link returning `EXDEV`
- open handle reads after unlink
- cached metadata after unlink
- regular-file `mknod` fallback and special-node `EPERM`
- create/write/read/release
- chmod/truncate through `setattr`
- xattr delegation when the host filesystem supports `user.*` xattrs

## Residual Gaps

The tests are still unit-level tests against the backend implementation. They do
not replace the later q35/microvm virtio-fs integration tests.

Known residual gaps:

- no stress/concurrency test yet
- no guest-kernel readdir offset mutation test
- no mounted-guest verification of kernel caching behavior
- stale host-backed inode retirement is documented as process-lifetime
  retention in `docker/composed-fs-operations.md`

These gaps should be handled by the q35 composed filesystem validation ticket
or by focused follow-ups if integration exposes a real workload failure.
