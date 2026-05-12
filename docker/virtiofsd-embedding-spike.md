# Virtiofsd Crate Embedding Feasibility Spike

Ticket: `wra-zgpp`

Date: 2026-05-12

## Outcome

Embedding upstream `virtiofsd` at the protocol boundary is feasible.

Validated locally:
- `virtiofsd` crate version: `1.13.3`
- `cargo`: `1.94.1`
- `rustc`: `1.94.1`
- a temporary compile probe successfully constructed:
  - a repo-owned `FileSystem` implementation
  - `VhostUserFsBackend<ProbeFs>`
  - `VhostUserDaemon`
  - a vhost-user Unix listener socket

This validates the intended reuse boundary:
- reuse upstream `virtiofsd` for FUSE request handling and vhost-user backend
  integration
- implement a repo-owned `filesystem::FileSystem`
- use `vhost-user-backend` and `vhost` the same way upstream `virtiofsd` does
- do not depend on private passthrough inode-store internals

## Crate Metadata

`cargo info virtiofsd` reported:

```text
virtiofsd
A virtio-fs vhost-user device daemon
version: 1.13.3
license: Apache-2.0 AND BSD-3-Clause
documentation: https://docs.rs/virtiofsd/1.13.3
homepage: https://virtio-fs.gitlab.io/
repository: https://gitlab.com/virtio-fs/virtiofsd
crates.io: https://crates.io/crates/virtiofsd/1.13.3
features:
 +default = [seccomp]
  seccomp = [dep:libseccomp-sys]
  xen     = [vhost-user-backend/xen, vhost/xen, vm-memory/xen]
```

Recommended dependency choice for the repo backend:

```toml
virtiofsd = { version = "1.13.3", default-features = false }
vhost = "0.13.0"
vhost-user-backend = "0.17.0"
vm-memory = { version = "0.16.0", features = ["backend-mmap", "backend-atomic"] }
```

Using `default-features = false` avoids pulling the `seccomp` binary feature
into the library embedding path. If the final backend needs seccomp, add it
deliberately rather than inheriting it accidentally.

## Public API Surface

The relevant public APIs are present in `virtiofsd 1.13.3`:

- `virtiofsd::filesystem::FileSystem`
- `virtiofsd::filesystem::SerializableFileSystem`
- `virtiofsd::filesystem::DirectoryIterator`
- `virtiofsd::vhost_user::VhostUserFsBackend`
- `virtiofsd::vhost_user::VhostUserFsBackendBuilder`

`VhostUserFsBackendBuilder::build` accepts:

```rust
F: FileSystem + SerializableFileSystem + Send + Sync + 'static
```

That is compatible with a repo-owned `ComposedFs`.

The upstream binary uses this pattern internally:

```rust
let fs_backend = Arc::new(
    VhostUserFsBackendBuilder::default()
        .set_thread_pool_size(thread_pool_size)
        .set_tag(tag)
        .build(fs)?;
);

let mut daemon = VhostUserDaemon::new(
    String::from("virtiofsd-backend"),
    fs_backend,
    GuestMemoryAtomic::new(GuestMemoryMmap::new()),
)?;

daemon.start(listener)?;
```

The local probe reproduced that shape with a minimal `ProbeFs`.

## Compile Probe

Temporary probe location during the spike:

```text
/tmp/virtiofsd-api-probe
```

The probe implemented only the trait skeleton needed to prove the API seam:

```rust
struct ProbeFs;

impl FileSystem for ProbeFs {
    type Inode = u64;
    type Handle = u64;
    type DirIter = EmptyDirIter;
}

impl SerializableFileSystem for ProbeFs {}
```

The probe built successfully with:

```sh
cargo build
```

It also created a vhost-user listener socket when run as:

```sh
timeout 3s /tmp/virtiofsd-api-probe/target/debug/virtiofsd-api-probe \
  /tmp/virtiofsd-api-probe.sock
```

Verified socket:

```text
srwxr-xr-x ... /tmp/virtiofsd-api-probe.sock
```

This proves backend construction and socket serving are viable. It does not
claim that the dummy filesystem is semantically useful.

## FileSystem Trait Implications

The `FileSystem` trait defaults most methods to `ENOSYS`, but `ComposedFs`
must implement real behavior for the operations identified by the filesystem
semantics spike.

Important trait requirement from upstream docs:
- every returned `Entry` increments lookup count
- `forget` decrements lookup count
- inodes with non-zero lookup count can still receive requests after unlink,
  rmdir, or rename
- open files/directories delay final forget until release/releasedir

This reinforces that inode lifetime and lookup bookkeeping are core backend
work, not an optional polish item.

`SerializableFileSystem` has default unsupported serialization/deserialization
methods, so the plan's no-live-migration v1 assumption is compatible with the
public API.

## Implementation Guidance

For `wra-saox`, scaffold the real backend around:

```rust
use std::sync::Arc;

use vhost::vhost_user::Listener;
use vhost_user_backend::VhostUserDaemon;
use virtiofsd::filesystem::{FileSystem, SerializableFileSystem};
use virtiofsd::vhost_user::VhostUserFsBackendBuilder;
use vm_memory::{GuestMemoryAtomic, GuestMemoryMmap};
```

The real process should:
- parse the composed mount manifest
- construct `ComposedFs`
- build `VhostUserFsBackendBuilder::default()`
- set thread pool size and tag from wrapper configuration
- create a `Listener` from the configured socket path
- call `VhostUserDaemon::new(...).start(listener)`

Keep the first backend binary small. Filesystem semantics belong in
`ComposedFs`, not in the process wiring.

## Risks

- `virtiofsd` is primarily an application crate that also exposes a library.
  Pin exact versions rather than using a loose dependency.
- The backend will need direct dependencies on `vhost`, `vhost-user-backend`,
  and `vm-memory`, not only `virtiofsd`.
- The public protocol seam is viable, but the filesystem implementation remains
  substantial and security-sensitive.
- The temporary probe did not implement a useful synthetic filesystem. That is
  intentional; the filesystem semantics baseline and backend implementation
  tickets should own that work.
