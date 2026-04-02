# Sandbox VM Runtime Contract

This document defines the target runtime contract for the VM-only rewrite of
`sandbox-wrap`.

It supersedes the earlier "host Bubblewrap sandbox plus guest Docker VM" model.
The supported end state is one isolation boundary only: the project-scoped VM.

## Scope

- Every normal wrapper launch starts a project-specific VM before the selected
  tool command runs.
- The selected tool command runs inside the guest, not on the host.
- Docker runs inside the same guest and is available to the tool there.
- The VM exists only for the lifetime of that wrapper invocation.
- Persistent Docker state is limited to the sparse data disk under
  `.sandbox/docker-vm/`.
- Persistent tool state is project-local under `.sandbox/`.
- v1 of the rewrite supports Linux hosts with KVM, QEMU, and `virtiofsd`.

## Host And Guest Responsibilities

### Host Wrapper

The host wrapper is only responsible for:

- resolving the project and selected tool
- preparing `.sandbox/` state and project-local runtime directories
- starting `virtiofsd`
- starting QEMU
- exposing any configured localhost port forwards
- passing the requested payload command into the guest
- wiring stdio, signals, and exit status between host and guest
- supervising the VM lifetime and tearing it down on exit

The host wrapper is not a second sandbox runtime. It must not run the agent
payload under Bubblewrap or any equivalent host-side namespace layer.

### Guest

The guest is responsible for:

- mounting the shared project workspace
- making the persistent guest home available
- starting `dockerd`
- applying guest network configuration
- making any configured auth/config shares visible at their guest paths
- launching the requested payload command

The guest is the only execution environment for the tool payload.

## Supported CLI Semantics

The VM-only contract intentionally removes flags that only existed to support
the old host-side Bubblewrap model.

### Flags That Stay

- `--project PATH`
- `--tool codex|copilot`
- `--no-net`
- `--docker-publish HOST:GUEST`
- `--ro PATH`
- `--rw PATH`
- `--gh`
- `--aws PROFILE`
- `--reset`
- extra command arguments after `--`

### Flags That Change Meaning

- `--no-net`
  - old meaning: disable host sandbox network namespace sharing
  - new meaning: start the VM with restricted guest networking and do not allow
    guest egress; localhost publish behavior remains a separate concern
- `--docker-publish HOST:GUEST`
  - old meaning: publish a guest container port when `--docker` was enabled
  - new meaning: publish a guest-side TCP port from the always-present VM

### Flags That Are Removed

- `--docker`
  - the VM is now the default execution model, so a separate opt-in flag is no
    longer part of the supported interface
- `--pass-env`

Retained mount flags should be reimplemented as explicit guest shares rather
than by preserving the old Bubblewrap bind machinery.

## Persistent And Transient State

All mutable state remains project-local under:

```text
.sandbox/
```

Runtime root:

```text
.sandbox/docker-vm/
```

Layout:

```text
.sandbox/
  home/
  docker-vm/
    docker-data.raw
    docker-data.meta.json
    lock
    run/
      state.json
      docker.sock
      virtiofs.sock
      qemu.pid
      virtiofsd.pid
      proxy.pid
      qemu.log
      virtiofsd.log
      proxy.log
      console.log
```

Rules:

- `.sandbox/home/` is the persistent guest home for the selected tool.
- `.sandbox/docker-vm/docker-data.raw` is the persistent sparse disk mounted in
  the guest at `/var/lib/docker`.
- `.sandbox/docker-vm/run/` is transient per-launch runtime state.
- A clean exit removes `.sandbox/docker-vm/run/` completely.
- `--reset` removes the entire `.sandbox/` tree, including guest home, Docker
  data, and all runtime logs, unless an active lock is held.

## Required Guest Shares

The VM-only contract assumes a deliberately small set of host inputs:

- project workspace
  - shared via `virtio-fs`
  - mounted inside the guest at the original absolute project path
  - optionally also available at `/workspace` as a compatibility alias
- persistent guest home
  - sourced from `.sandbox/home/`
  - mounted inside the guest at the guest user's home path
- tool/auth/config material
  - only the minimum required host-backed inputs should be exposed
  - examples include tool auth state, Docker client config, GitHub auth, and
    AWS credentials when explicitly requested
- arbitrary user-requested path shares
  - `--ro PATH` exposes a host path read-only inside the guest at the same
    absolute path
  - `--rw PATH` exposes a host path read-write inside the guest at the same
    absolute path
  - these are supported because arbitrary path mounts are a real requirement,
    but they should be implemented as a small explicit guest-share mechanism
    rather than as a full host-session recreation

The contract does not require recreating a full host home directory inside the
guest.

## Lifecycle Contract

The VM is not a background project daemon. One wrapper launch owns one VM
instance.

State machine:

- `absent`: `.sandbox/docker-vm/run/` does not exist and no lock is held
- `starting`: lock is held, runtime files are being created, and the guest is
  booting
- `running`: the guest services are ready and the payload is eligible to start
- `payload_running`: the payload is active inside the guest
- `stopping`: payload exit or host termination has triggered teardown
- `failed`: startup or supervision failed before clean teardown

Transitions:

- `absent -> starting`: wrapper acquires the lock, removes stale runtime files,
  creates a new runtime directory, and starts helper processes
- `starting -> running`: QEMU, `virtiofsd`, guest init, and `dockerd` are up,
  and the payload launch channel is ready
- `running -> payload_running`: the wrapper asks the guest to launch the
  requested command
- `payload_running -> stopping`: the payload exits or the host wrapper receives
  a terminating signal
- `starting -> failed` or `payload_running -> failed`: required helper process
  exits unexpectedly or readiness/control path fails
- `stopping -> absent`: teardown completes

## Startup Sequence

Required sequence:

1. Resolve project and selected tool.
2. Ensure `.sandbox/`, `.sandbox/home/`, and `.sandbox/docker-vm/` exist.
3. Acquire an exclusive non-blocking lock on `.sandbox/docker-vm/lock`.
4. Remove stale `.sandbox/docker-vm/run/` from a previous failed or aborted run.
5. Ensure `docker-data.raw` and `docker-data.meta.json` exist.
6. Create `.sandbox/docker-vm/run/`.
7. Start `virtiofsd`.
8. Start QEMU with:
   - read-only root disk
   - persistent Docker data disk
   - `virtio-fs` workspace sharing
   - user-mode networking
   - the guest control path needed to launch the payload
   - any requested localhost port forwards
9. Wait for guest init and `dockerd` readiness.
10. Launch the requested payload inside the guest.
11. Supervise guest payload exit and VM teardown from the host wrapper.

## Payload Control Contract

The host wrapper must have a narrow, explicit way to launch a command in the
guest and observe its result.

Required properties:

- it must carry the exact requested command
- it must support interactive stdio
- it must propagate termination signals
- it must return the guest payload exit code to the host wrapper
- it must not require a second full RPC/control framework beyond what the
  payload launch path needs

The exact transport is an implementation detail for downstream tickets. The
runtime contract only requires the behavior above.

## Readiness Checks

The VM is considered ready for payload launch only when all of these are true:

- QEMU is alive
- `virtiofsd` is alive
- required guest mounts are in place
- `dockerd` is ready inside the guest
- the payload launch/control path is ready

Readiness checks must not depend on a host-installed `docker` CLI binary.

Timeouts:

- boot/readiness timeout: 60 seconds
- graceful shutdown timeout: 10 seconds before forcible termination

## Shutdown And Cleanup

Shutdown begins when the guest payload exits or the host wrapper receives a
termination signal.

Required order:

1. Mark `state.json` as `stopping`.
2. Stop or interrupt the guest payload if it is still active.
3. Terminate QEMU, `virtiofsd`, and any host-side helper process that remains
   necessary in the VM-only design.
4. Wait up to the configured timeout before force-killing remaining processes.
5. Remove transient files under `.sandbox/docker-vm/run/`.
6. Release the project lock.

Clean exit removes `.sandbox/docker-vm/run/` entirely.

Failed startup leaves logs in place until the next launch or `--reset`.

## Concurrency And Reset Rules

v1 allows at most one active VM-backed sandbox per project.

- the lock is held for the full lifetime of the owning wrapper process
- a second launch for the same project must fail fast
- different projects remain isolated by separate `.sandbox/` trees

`--reset` behavior:

- if the project lock is held, `--reset` must fail with a clear error
- otherwise `--reset` removes the full `.sandbox/` directory
