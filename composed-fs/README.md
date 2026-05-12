# Agent VM Composed FS Backend

This is the scaffold for the repo-local composed `virtio-fs` backend.

It currently:
- parses schema version `1` from `.sandbox/docker-vm/run/composed-fs-manifest.json`
- builds a synthetic/overlay namespace from manifest `guest_path` entries
- opens mount roots as host fds and exposes host-backed lookup/getattr/readdir
- reuses backend inodes for host nodes using `(mount, dev, ino)`
- tracks lookup counts for entries returned by `lookup`
- starts a vhost-user socket using upstream `virtiofsd` protocol-boundary APIs

The backend deliberately does not yet implement the full write/read operation
surface. That belongs to the follow-up operation-surface ticket.

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
