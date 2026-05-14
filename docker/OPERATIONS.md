# VM Operations

This document describes the current VM-only runtime operated by the Rust
`agentvm-frontend` binary.

## Summary

- The agent payload always runs inside a project-scoped QEMU microvm.
- Docker runs inside the same guest and is available at
  `unix:///var/run/docker.sock`.
- The host does not run the payload under Bubblewrap.
- Filesystem sharing is served by embedded Rust composed-fs instances.
- Guest networking is enforced by the Rust userspace vmnet gateway over QEMU
  `-netdev stream`, not by QEMU user networking or `hostfwd`.
- Persistent state is project-local under `.sandbox/`.

## Host Prerequisites

Required on the host:

- Linux with `/dev/kvm`
- `qemu-system-x86_64`
- `mkfs.ext4`
- Rust/Cargo for local development runs

The appliance artifacts must exist under `docker/out/`. Build them with:

```sh
docker/refresh-pins.sh
sudo docker/build-appliance.sh
```

## Runtime Layout

```text
.sandbox/
  home/
  docker-vm/
    docker-data.raw
    lock
    run/
      state.json
      console.log
      qemu.log
      vmnet-events.log
      guest-dockerd.log
      guest-socket-bridge.log
      guest-payload-server.log
      docker.sock
      virtiofs.sock
      guest-config.sock
      composed-fs-manifest.json
      config-fs-manifest.json
      guest-config/composed-binds.json
```

Meaning:

- `.sandbox/home/` is the persistent guest `$HOME`.
- `docker-data.raw` is the persistent sparse ext4 disk mounted at
  `/var/lib/docker`.
- `lock` prevents concurrent VM launches for the same project.
- `run/` contains the current launch's manifests, sockets, state, and logs.

## Lifecycle

Normal flow:

1. The Rust frontend resolves the project, tool, policy, and guest shares.
2. It acquires `.sandbox/docker-vm/lock`.
3. It creates `.sandbox/home/`, `.sandbox/docker-vm/`, and the selected
   run directory.
4. It creates and formats `docker-data.raw` on first use.
5. It writes composed-fs and config-fs manifests.
6. It starts embedded composed-fs servers for workspace/config sharing.
7. It starts the Rust vmnet gateway and any requested host listeners.
8. It starts QEMU with microvm, read-only rootfs, Docker data disk,
   virtio-fs devices, and stream networking.
9. It waits for the guest payload control path.
10. It launches the requested payload in the guest.
11. It forwards stdio, signals, terminal resize events, and guest exit status.
12. It terminates QEMU and releases the project lock.

Failed launches leave `run/` logs and manifests available for inspection.

## Networking

The guest NIC connects to the Rust vmnet gateway through QEMU stream frames.

- Direct `launch` can use `--allow-public-internet` for public egress.
- Wrapper mode enables public egress unless `--no-net` is supplied.
- `--no-net` denies guest egress while preserving frontend control listeners.
- `--docker-publish HOST:GUEST` maps to frontend `--publish HOST:GUEST`.
- Published ports are implemented by frontend-owned loopback listeners.

## Reset

`--reset` removes `.sandbox/`, including guest home and Docker state, unless
the project VM lock is currently held.

## Verification

Non-KVM checks:

```sh
cargo test --manifest-path vm-frontend/Cargo.toml --offline
```

KVM self-test:

```sh
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- \
  self-test \
  --project "$PWD" \
  --run-dir "$PWD/.sandbox/docker-vm/self-test" \
  --artifact-manifest "$PWD/docker/out/artifact-manifest.json" \
  --qemu /usr/bin/qemu-system-x86_64 \
  --publish-payload-port 12079
```

Expected output includes:

```text
self-test: published payload port 12079 ok
self-test: payload-start
docker-run-ok
bind-ok
self-test: payload-ok
self-test: ok
```

## Troubleshooting

Inspect:

- `.sandbox/docker-vm/run/state.json`
- `.sandbox/docker-vm/run/qemu.log`
- `.sandbox/docker-vm/run/console.log`
- `.sandbox/docker-vm/run/vmnet-events.log`
- `.sandbox/docker-vm/run/guest-dockerd.log`
- `.sandbox/docker-vm/run/guest-socket-bridge.log`
- `.sandbox/docker-vm/run/guest-payload-server.log`

Common causes:

- Missing `/dev/kvm`: run on a KVM-capable Linux host.
- Missing appliance artifacts: rebuild with `sudo docker/build-appliance.sh`.
- Docker pull failures: inspect `vmnet-events.log` and guest Docker logs.
- Concurrent launch: wait for the active VM process or use `--reset` only
  after the lock is released.
