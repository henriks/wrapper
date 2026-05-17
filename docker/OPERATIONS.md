# VM Operations

This document describes the current VM-only runtime operated by the Rust
`agentvm-frontend` binary.

## Summary

- The agent payload always runs inside a project-scoped QEMU microvm.
- Docker runs inside the same guest and is available to payloads through the
  guest socket bridge at `tcp://127.0.0.1:1075`.
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
  docker-vm/
    state.raw
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

- `state.raw` is the persistent sparse ext4 disk backing the guest root
  overlay. Guest writes under normal paths, including `$HOME`, `/usr/local`,
  package caches, and `/var/lib/docker`, persist there.
- `lock` prevents concurrent VM launches for the same project.
- `run/` contains the current launch's manifests, sockets, state, and logs.

## Lifecycle

Normal flow:

1. The Rust frontend resolves the project, tool, policy, and guest shares.
2. It acquires `.sandbox/docker-vm/lock`.
3. It creates `.sandbox/docker-vm/` and the selected run directory.
4. It creates and formats `state.raw` on first use.
5. It writes composed-fs and config-fs manifests.
6. It starts embedded composed-fs servers for workspace/config sharing.
7. It starts the Rust vmnet gateway and any requested host listeners.
8. It starts QEMU with microvm, read-only lower rootfs, root overlay state
   disk, virtio-fs devices, and stream networking.
9. It waits for guest service readiness: Docker `_ping` succeeds inside the
   guest and the payload control path responds.
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
self-test: home-ok
self-test: uid-ok
self-test: gid-ok
self-test: home-dir-ok
self-test: cwd-ok
self-test: sqlite-home-smoke
self-test: sqlite-concurrency-smoke
docker-run-ok
bind-ok
self-test: payload-ok
self-test: ok
```

During the SQLite concurrency portion, the self-test uses
`.agentvm-self-test-sqlite/state.sqlite` in the workspace. A valid run has host
and guest rows in that database and `PRAGMA integrity_check = ok`. If the run
fails after creating the directory, keep it for triage until the database has
been inspected.

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
- Payload behavior that does not match current guest source: rebuild
  `docker/out/rootfs.raw`; the generated artifacts may contain an older
  `agentvm-payload-server`.
- Concurrent launch: wait for the active VM process or use `--reset` only
  after the lock is released.
