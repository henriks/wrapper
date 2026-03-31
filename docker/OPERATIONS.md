# Docker VM Operations

This document describes the current VM-backed Docker behavior implemented in
`sandbox-wrap`.

## Summary

- `--docker` means VM-backed Docker only.
- The wrapper starts a project-local Cloud Hypervisor VM before launching
  `bwrap`.
- The sandbox sees only the project-local Docker socket at
  `/run/docker.sock` and `/var/run/docker.sock`.
- The wrapper shuts the VM down when the sandbox exits.
- Persistent Docker state lives in `.sandbox/docker-vm/docker-data.raw`.

## Host Prerequisites

Required on the host:

- Linux
- `bwrap`
- `mise`
- `cloud-hypervisor`
- `virtiofsd`
- `mkfs.ext4`
- `/dev/kvm`
- `/dev/vhost-vsock`

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
    ch-api.sock
    ch-vsock.sock
    virtiofs.sock
    cloud-hypervisor.pid
    virtiofsd.pid
    proxy.pid
    cloud-hypervisor.log
    virtiofsd.log
    proxy.log
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
4. It starts Cloud Hypervisor and creates/boots the VM from
   `docker/out/artifact-manifest.json`.
5. It starts the host-side proxy at `.sandbox/docker-vm/run/docker.sock`.
6. It waits for Docker `GET /_ping` to succeed through that socket.
7. It launches `bwrap` as a child process.
8. When the sandbox exits, the wrapper shuts the VM and helper processes down
   and removes `.sandbox/docker-vm/run/`.

Failure flow:

- If startup fails, the wrapper leaves `.sandbox/docker-vm/run/` and the log
  files in place for inspection.
- The next `--docker` launch removes that stale `run/` directory before
  retrying.

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

- `Missing required Docker VM host tools: cloud-hypervisor, virtiofsd`
  Install the missing host binaries before using `--docker`.
- `/dev/kvm is required for Docker VM support`
  KVM is not available on the host.
- `/dev/vhost-vsock is required for Docker VM support`
  Host vsock support is missing.
- `Docker VM artifact manifest is missing`
  Run `docker/build-appliance.sh`.
- Cloud Hypervisor dies with `SIGSYS` or a seccomp violation on the first API
  request
  The wrapper currently starts Cloud Hypervisor with `--seccomp false` by
  default for compatibility. Override this with
  `SANDBOX_WRAP_CLOUD_HYPERVISOR_SECCOMP=true` only if seccomp is known to work
  on the host.
- Docker startup timeout
  Inspect:
  - `.sandbox/docker-vm/run/cloud-hypervisor.log`
  - `.sandbox/docker-vm/run/virtiofsd.log`
  - `.sandbox/docker-vm/run/proxy.log`
  - `.sandbox/docker-vm/run/state.json`

Manual cleanup:

- If no Docker sandbox is active for the project, `--reset` is the supported
  cleanup path.
- If you need to inspect a failed launch before resetting, preserve
  `.sandbox/docker-vm/run/` and read the logs there.
