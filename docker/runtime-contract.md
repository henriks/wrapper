# Docker VM Runtime Contract

This document defines the host-side runtime contract for VM-backed Docker in
`sandbox-wrap`.

## Scope

- `--docker` starts a project-specific Docker VM before entering `bwrap`.
- The VM exists only for the lifetime of that sandbox process.
- The VM is terminated when the sandbox exits normally or due to a signal.
- Persistent Docker state is limited to the sparse data disk under
  `.sandbox/docker-vm/`.
- v1 supports Linux hosts with KVM, Cloud Hypervisor, `virtiofsd`, and
  `vhost-vsock`.

## Runtime Root

All VM state lives under:

```text
.sandbox/docker-vm/
```

Layout:

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

Rules:

- `docker-data.raw` is the persistent sparse raw disk mounted in the guest at
  `/var/lib/docker`.
- `docker-data.meta.json` is persistent metadata for the disk. It must record
  at least the format version, logical size in bytes, and filesystem type.
- `lock` is the project-level lock file used to prevent concurrent Docker VM
  launches for the same project.
- `run/` contains transient files for the active sandbox run.
- A clean sandbox exit removes `run/` completely.
- `--reset` removes the entire `.sandbox/` tree, including `docker-data.raw`
  and everything under `run/`.

## Lifecycle Contract

The VM is not a background project daemon. One `--docker` sandbox launch owns
one VM instance.

State machine:

- `absent`: `.sandbox/docker-vm/run/` does not exist and no lock is held.
- `starting`: lock is held, `run/` exists, child processes are being started,
  `state.json` has `"status": "starting"`.
- `running`: host proxy socket exists, Docker readiness check passes, and
  `state.json` has `"status": "running"`.
- `stopping`: shutdown has started, `state.json` has `"status": "stopping"`.
- `failed`: startup or runtime supervision failed before clean teardown;
  `state.json` has `"status": "failed"`.

Transitions:

- `absent -> starting`: wrapper acquires `lock`, removes stale `run/`, creates
  a new `run/`, writes initial `state.json`, and starts helper processes.
- `starting -> running`: Cloud Hypervisor, `virtiofsd`, and the host proxy are
  all up and the Docker readiness check succeeds.
- `starting -> failed`: any required child process exits early, VM boot times
  out, or readiness check fails.
- `running -> stopping`: sandbox process exits, parent receives `SIGINT`,
  `SIGTERM`, `SIGHUP`, or wrapper detects loss of the sandbox child.
- `stopping -> absent`: guest shutdown and host cleanup complete.
- `failed -> absent`: next launch or `--reset` removes stale runtime files.

## Startup Sequence

For `--docker`, startup happens before `bwrap` is launched.

Required sequence:

1. Ensure `.sandbox/` exists.
2. Ensure `.sandbox/docker-vm/` exists.
3. Acquire an exclusive non-blocking lock on `.sandbox/docker-vm/lock`.
4. If the lock is already held, fail immediately with a clear error that a
   Docker sandbox is already active for this project.
5. Remove any stale `.sandbox/docker-vm/run/` directory left by a prior crash.
6. Ensure `docker-data.raw` and `docker-data.meta.json` exist.
7. Create `.sandbox/docker-vm/run/`.
8. Start `virtiofsd`.
9. Start Cloud Hypervisor and create the VM through its API socket.
10. Start the host Unix-socket proxy.
11. Wait for Docker readiness.
12. Launch `bwrap`, mounting only `.sandbox/docker-vm/run/docker.sock` at
    `/run/docker.sock` and `/var/run/docker.sock`.

Important implication for implementation:

- The wrapper cannot `execvp()` directly into `bwrap` for `--docker`.
- The wrapper must remain the supervisor process so it can hold the lock, watch
  the sandbox child, and guarantee teardown on exit.

## Readiness Check

The VM is considered ready only when all of these are true:

- `.sandbox/docker-vm/run/docker.sock` exists.
- The host proxy process is alive.
- An HTTP `GET /_ping` over the Unix socket returns success from the guest
  Docker daemon.

The readiness probe must use the project-local Unix socket and must not depend
on a host `docker` CLI binary being installed.

Timeouts:

- Boot/readiness timeout: 60 seconds.
- Graceful shutdown timeout: 10 seconds before forcible process termination.

## Shutdown And Cleanup

Shutdown begins when the sandbox child exits or the wrapper receives a
termination signal.

Required order:

1. Mark `state.json` as `stopping`.
2. Ask Cloud Hypervisor to shut the VM down through its API socket.
3. Wait up to 10 seconds for Cloud Hypervisor to exit.
4. If the VM or helper processes are still alive, send `SIGTERM`.
5. If they still remain, send `SIGKILL`.
6. Remove `docker.sock`, `ch-api.sock`, `ch-vsock.sock`, `virtiofs.sock`, all
   PID files, and the rest of `run/`.
7. Release the project lock.

Clean exit removes `run/` entirely.

Failed startup keeps `run/` and logs in place so the next operator action can
inspect them. The next `--docker` launch must delete that stale `run/` before
retrying.

## Concurrency And Reset Rules

v1 allows at most one active `--docker` sandbox per project.

- The `lock` file is held for the full lifetime of the wrapper process that
  owns the Docker VM.
- A second `--docker` launch for the same project must fail fast; it does not
  attach to an existing VM.
- Different projects use different `.sandbox/docker-vm/` directories and are
  isolated.

`--reset` behavior:

- If `.sandbox/docker-vm/lock` is currently held, `--reset` must fail with a
  clear error instead of removing active VM state.
- If no lock is held, `--reset` removes the full `.sandbox/` directory.

## Downstream Contract For Later Tickets

- The image-build ticket must produce immutable appliance artifacts consumable
  by the launcher, but those artifacts are not stored in `.sandbox/docker-vm/`.
- The launcher ticket must implement the supervisor model described here rather
  than a detached background daemon.
- The socket-proxy ticket must expose exactly one host-visible socket at
  `.sandbox/docker-vm/run/docker.sock`.
- The `sandbox-wrap` integration ticket must replace direct host Docker socket
  mounting and must run `bwrap` as a supervised child process when `--docker`
  is enabled.
