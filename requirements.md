# Sandbox Wrapper Requirements

## Overview

The supported sandbox model is a project-scoped QEMU microvm managed by the
Rust `agentvm-frontend` binary. The selected agent payload runs inside the
guest. Docker also runs inside that guest and is available to the payload by
default.

There is no supported host-side Bubblewrap execution path.

## Entry Points

The Rust binary supports explicit frontend subcommands and explicit wrapper
startup through `wrap`:

```text
agentvm-frontend launch ...
agentvm-frontend self-test ...
agentvm-frontend wrap ...
```

The executable name is not part of wrapper behavior. `codex-wrap` and
`copilot-wrap` aliases are not supported entrypoints, and the selected tool must
come from explicit flags or the interactive TUI startup flow.

Wrapper arguments after `--` are passed to the selected tool as tool arguments.

## Supported Wrapper Flags

```text
--project PATH
--tool codex|copilot
--no-net
--docker-publish HOST:GUEST
--ro PATH
--rw PATH
--gh
--aws PROFILE
--reset
```

Removed flags:

```text
--docker
--docker-machine
--pass-env
```

The VM is always used, so `--docker` is no longer meaningful. Arbitrary
environment passthrough is intentionally not part of the VM-only contract;
supported auth/config flows are explicit.

## State Layout

All mutable state is project-local under `.sandbox/`:

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

`.sandbox/home/` is the project-local backing store for the guest user's
natural home path. The sparse `docker-data.raw` disk is mounted in the guest at
`/var/lib/docker`. `run/` contains per-launch sockets, manifests, state, and
diagnostic logs.

`--reset` removes `.sandbox/` unless the project VM lock is held.

## Guest Filesystem Contract

The frontend writes composed-fs manifests and serves them directly from Rust.
The guest mounts the composed export and binds the declared entries into place.

Required/default guest shares:

- The project workspace is mounted read-write at its original absolute path.
- `/workspace` is a compatibility alias for the project path.
- `.sandbox/home/` is mounted at the host user's natural home path in the guest;
  the guest does not see `.sandbox/home` as `$HOME`.
- Selected tool state is shared deliberately, not by mounting broad host `$HOME`.
- Enabling Codex in the wrapper/TUI setup exposes Codex state, currently
  `~/.codex`, as writable tool state at the same absolute path in the guest.
- `~/.docker` is shared as writable tool state when present.
- `--gh` shares `~/.config/gh` read-only and forwards `GH_TOKEN` when available.
- `--ro PATH` exposes a required read-only host path at the same guest path.
- `--rw PATH` exposes a required read-write host path at the same guest path.

The VM-only design does not mount broad host system directories into the guest.
The guest root filesystem is the appliance image.

## Payload Environment

For tool launches, the frontend sets:

- `HOME=<host home path>`
- `USER` and `LOGNAME`
- `TERM` and `LANG`
- XDG base directories under guest `$HOME`
- `PATH` with guest-home local bins plus standard guest system paths
- `TMPDIR=/tmp`
- `DOCKER_HOST=tcp://127.0.0.1:1075`
- `AGENTVM_UID` and `AGENTVM_GID` carrying the host uid/gid used for payload
  privilege drop inside the guest

Credential leakage controls:

- `SSH_AUTH_SOCK` is cleared.
- `GIT_CONFIG_GLOBAL=/dev/null`
- `AWS_SHARED_CREDENTIALS_FILE=/dev/null`
- `AWS_CONFIG_FILE=/dev/null`
- `GOOGLE_APPLICATION_CREDENTIALS` is cleared.
- `KUBECONFIG=/dev/null`

`--aws PROFILE` obtains credentials on the host with
`aws configure export-credentials --profile PROFILE --format process` and
injects the resulting temporary credential environment into the guest payload.

## Networking

QEMU uses `-netdev stream` over a Unix socket. The Rust vmnet gateway owns the
guest network boundary in userspace:

- default policy is deny-by-default unless an allow profile is selected
- `--allow-public-internet` is used by direct `launch` for public egress
- wrapper mode enables public egress unless `--no-net` is supplied
- `--no-net` denies guest egress while preserving frontend control channels
- `--docker-publish HOST:GUEST` maps to a frontend-owned host listener, not
  QEMU `hostfwd`

Unsupported protocols and bypass paths are denied by policy.

## Verification

Non-KVM coverage:

```sh
cargo test --manifest-path vm-frontend/Cargo.toml --offline
```

Real KVM self-test:

```sh
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- \
  self-test \
  --project "$PWD" \
  --run-dir "$PWD/.sandbox/docker-vm/self-test" \
  --artifact-manifest "$PWD/docker/out/artifact-manifest.json" \
  --qemu /usr/bin/qemu-system-x86_64 \
  --publish-payload-port 12079
```

The self-test boots the VM, validates the payload channel, optionally validates
a published host port, checks guest `$HOME` and workspace sharing, verifies the
Docker daemon, runs a container, and confirms a workspace bind mount from inside
that container.
