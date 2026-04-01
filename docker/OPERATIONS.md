# Docker VM Operations

This document describes the current VM-backed Docker behavior implemented in
`sandbox-wrap`.

## Summary

- `--docker` means VM-backed Docker only.
- The wrapper starts a project-local QEMU VM before launching `bwrap`.
- The sandbox sees only the project-local Docker socket at
  `/run/docker.sock` and `/var/run/docker.sock`.
- Outbound guest networking uses QEMU user-mode networking, so no `sudo`,
  TAP, `iptables`, or `nft` setup is required.
- `--docker-publish HOST:GUEST` exposes a guest TCP port on
  `127.0.0.1:HOST` through QEMU `hostfwd`.
- The wrapper shuts the VM down when the sandbox exits.
- Persistent Docker state lives in `.sandbox/docker-vm/docker-data.raw`.

## Host Prerequisites

Required on the host:

- Linux
- `bwrap`
- `mise`
- `qemu-system-x86_64`
- `virtiofsd`
- `mkfs.ext4`
- `/dev/kvm`

The appliance artifacts must also exist under `docker/out/`. Build them with:

```bash
docker/refresh-pins.sh
sudo docker/build-appliance.sh
```

## Runtime Layout

The project-local runtime lives under:

```text
.sandbox/docker-vm/
```

Files:

```text
.sandbox/docker-vm/
  docker-data.raw
  docker-data.meta.json
  lock
  run/
    state.json
    docker.sock
    virtiofs.sock
    qemu.pid
    virtiofsd.pid
    qemu.log
    virtiofsd.log
```

Meaning:

- `docker-data.raw` is the persistent sparse ext4 disk mounted inside the
  guest at `/var/lib/docker`.
- `docker-data.meta.json` records the disk metadata written by the launcher.
- `lock` prevents more than one active `--docker` sandbox for the same
  project.
- `run/` is transient runtime state for the current sandbox process.

## Lifecycle

Normal flow:

1. `sandbox-wrap --docker` acquires the project lock.
2. It creates `.sandbox/docker-vm/run/`.
3. It starts `virtiofsd`.
4. It starts QEMU from `docker/out/artifact-manifest.json`.
5. QEMU exposes `.sandbox/docker-vm/run/docker.sock` as a host Unix socket
   that forwards to the guest Docker bridge.
6. The wrapper waits for Docker `GET /_ping` to succeed through that socket.
7. If `--docker-publish` was used, the same QEMU user-network backend exposes
   those forwarded TCP ports on `127.0.0.1`.
8. It launches `bwrap` as a child process.
9. When the sandbox exits, the wrapper terminates the VM and helper processes
   and removes `.sandbox/docker-vm/run/`.

Failure flow:

- If startup fails, the wrapper leaves `.sandbox/docker-vm/run/` and the log
  files in place for inspection.
- The next `--docker` launch removes that stale `run/` directory before
  retrying.

## Networking

The Docker VM always has a QEMU user-network NIC because that is also how the
host Docker socket is exposed into the guest.

- With normal `--docker`, outbound guest networking is enabled.
- With `--docker --no-net`, the VM still boots with the same internal NIC and
  Docker socket forward, but guest egress is restricted by QEMU user-network.
- `--docker-publish HOST:GUEST` adds extra QEMU `hostfwd` rules. These are not
  available with `--no-net`.

## Reset Behavior

`--reset` removes the full `.sandbox/` directory, including:

- `.sandbox/docker-vm/docker-data.raw`
- `.sandbox/docker-vm/docker-data.meta.json`
- `.sandbox/docker-vm/run/`

Safety rule:

- If `.sandbox/docker-vm/lock` is currently held by an active Docker sandbox,
  `--reset` fails instead of removing active VM state.

## Troubleshooting

Common failures:

- `Missing required Docker VM host tools: qemu-system-x86_64, virtiofsd`
  Install the missing host binaries before using `--docker`.
- `/dev/kvm is required for Docker VM support`
  KVM is not available on the host.
- `Docker VM manifest is missing the vm stanza`
  Rebuild the appliance with `docker/build-appliance.sh`.
- `Docker VM manifest is missing guest.docker_tcp_port`
  Rebuild the appliance with `docker/build-appliance.sh`.
- Docker startup timeout
  Inspect:
  - `.sandbox/docker-vm/run/qemu.log`
  - `.sandbox/docker-vm/run/virtiofsd.log`
  - `.sandbox/docker-vm/run/state.json`
- Registry pull or container egress failures
  Inspect `.sandbox/docker-vm/run/qemu.log` and the guest logs mirrored into
  the workspace under `.sandbox/docker-vm/run/guest-*.log`.

Manual cleanup:

- If no Docker sandbox is active for the project, `--reset` is the supported
  cleanup path.
- If you need to inspect a failed launch before resetting, preserve
  `.sandbox/docker-vm/run/` and read the logs there.
