# Validation Workflow

This project has four validation tiers. Normal development should run the fast
tier. Stress and live tiers are opt-in so they can be run when changing the VM
frontend, composed filesystem, network gateway, or wrapper contract.

## Fast Offline

Run on every relevant code change:

```sh
vm-frontend/validate.sh fast
```

Equivalent commands:

```sh
cargo test --manifest-path composed-fs/Cargo.toml --offline
cargo test --manifest-path vm-frontend/Cargo.toml --offline
```

Expected runtime is a few seconds on a warm build. This tier must not require
network access, root, `/dev/kvm`, QEMU, Docker, or rebuilt appliance artifacts.

## Stress And Property

Run before closing validation tickets or after touching protocol, path, or policy
logic:

```sh
vm-frontend/validate.sh stress
```

Equivalent commands:

```sh
cargo test --manifest-path composed-fs/Cargo.toml --offline proptest_flat_file_operation_sequences -- --ignored --nocapture
cargo test --manifest-path composed-fs/Cargo.toml --offline stress_seeded_flat_file_operation_sequences -- --ignored --nocapture
cargo test --manifest-path vm-frontend/Cargo.toml --offline dns_proxy_stress -- --ignored --nocapture
```

The composed-fs property test uses `proptest` with shrinking. The fixed-seed
filesystem regression uses seed `0x5eedf17e20260514`. The DNS stress regression
uses seed `0xd15c20260514`. Failures print either the minimized generated input
or an operation trace; paste that trace into the fixing ticket before closing it.

## Live KVM

Run after rebuilding appliance artifacts and before closing live frontend
contract work:

```sh
vm-frontend/validate.sh live
```

Equivalent command:

```sh
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- \
  self-test \
  --project "$PWD" \
  --run-dir "$PWD/.sandbox/docker-vm/self-test" \
  --artifact-manifest "$PWD/docker/out/artifact-manifest.json" \
  --qemu /usr/bin/qemu-system-x86_64 \
  --image alpine:3.22 \
  --publish-payload-port 12079
```

Prerequisites:

- `/dev/kvm` visible to the process.
- `qemu-system-x86_64` installed.
- `docker/out/artifact-manifest.json`, `docker/out/vmlinuz`,
  `docker/out/initrd.img`, and `docker/out/rootfs.raw` built from the current
  guest scripts.
- Host network access if the selected Docker image is not already present in
  the project-local Docker data disk.

The live tier validates the real QEMU stream network, payload-control listener,
published payload port, composed virtiofs workspace, guest config filesystem,
MITM CA bundle exposure without private key exposure, guest DNS lookup, Docker
CLI, and Docker bind-mounted workspace.

## Artifacts And Triage

On live failures, collect `.sandbox/docker-vm/self-test/state.json` first. It
points to:

- `qemu.log`
- `console.log`
- `vmnet-events.log`
- `composed-fs-manifest.json`
- `config-fs-manifest.json`
- `guest-config/composed-binds.json`

`vmnet-events.log` is the primary network artifact. It records DNS decisions,
UDP denials, unsupported protocol classifications, TCP policy/setup failures,
HTTP/TLS proxy events, TLS MITM failures, upstream failures, and host-ingress
events. It must not contain CA private key material.

For filesystem failures, preserve the generated manifests and any operation
trace from the failing test. The trace is part of the repro and should be added
to the ticket note.

## CI Policy

Recommended split:

- Per-commit CI: `vm-frontend/validate.sh fast`.
- Pre-merge/manual CI: `vm-frontend/validate.sh all-local`.
- Nightly or host-only CI: `vm-frontend/validate.sh live` on a runner with KVM
  and rebuilt appliance artifacts.

When adding validation, update `validation-matrix.md`, add the exact command to
this file if it creates a new tier or named filter, and record the outcome in
the relevant `tk` ticket before closing it.
