# Agent VM Composed FS Backend

This is the scaffold for the repo-local composed `virtio-fs` backend.

It currently:
- parses schema version `1` from `.sandbox/docker-vm/run/composed-fs-manifest.json`
- builds a synthetic/overlay namespace from manifest `guest_path` entries
- opens mount roots as host fds and exposes host-backed lookup/getattr/readdir
- reuses backend inodes for host nodes using `(mount, dev, ino)`
- tracks lookup counts for entries returned by `lookup`
- supports the initial host-backed operation slice: open/create/read/write,
  mkdir/unlink/rmdir/rename/link/symlink/readlink, statfs/access/lseek, and
  regular-file `mknod`
- starts a vhost-user socket using the vendored `virtiofsd` protocol-boundary
  APIs
- exposes the same backend as a Rust library for the VM frontend

`virtiofsd` is vendored under `third_party/virtiofsd` because the upstream
`1.13.3` `FileSystem` trait does not expose full FUSE lock request details.
See `third_party/virtiofsd/README.agentvm.md` before updating that dependency.

The backend deliberately does not yet implement the full v1 operation surface.
Remaining work is documented in `docker/composed-fs-operations.md`.

Build:

```sh
cargo build --manifest-path composed-fs/Cargo.toml
```

Run:

```sh
composed-fs/target/debug/agentvm-composed-fs \
  --manifest .sandbox/docker-vm/run/composed-fs-manifest.json \
  --socket-path .sandbox/docker-vm/run/virtiofs.sock \
  --tag agentvm
```

Embed:

```rust
use std::path::PathBuf;

use agentvm_composed_fs::{serve_vhost_user_fs, ServeConfig};

serve_vhost_user_fs(ServeConfig {
    manifest: PathBuf::from(".sandbox/docker-vm/run/composed-fs-manifest.json"),
    socket_path: PathBuf::from(".sandbox/docker-vm/run/virtiofs.sock"),
    tag: "agentvm".to_string(),
    thread_pool_size: 1,
})?;
```

`serve_vhost_user_fs` is intentionally blocking: it owns the vhost-user listener
and waits for the daemon to exit. A frontend supervisor should run it in its own
thread or blocking task and treat the existing `agentvm-composed-fs` binary as a
migration bridge, not a second filesystem implementation.
