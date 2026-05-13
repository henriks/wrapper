# q35 Composed Filesystem Integration

Ticket: `wra-0bcu`

Date: 2026-05-13

## Switch

The composed filesystem path is explicit and non-default:

```sh
sandbox-wrap --docker --docker-composed-fs ...
```

Without `--docker-composed-fs`, the wrapper keeps the existing q35 path:

- primary project `virtiofsd`
- tiny readonly guest config `virtiofsd`
- one supplemental per-share `virtiofsd` per extra guest share

## Runtime Files

Composed mode writes:

```text
.sandbox/docker-vm/run/composed-fs-manifest.json
.sandbox/docker-vm/run/guest-config/composed-binds.json
.sandbox/docker-vm/run/virtiofs.sock
.sandbox/docker-vm/run/virtiofsd.pid
.sandbox/docker-vm/run/virtiofsd.log
```

The pid/log filenames intentionally reuse the existing primary virtio-fs paths
so the process supervision and failure reporting path remains compatible. In
composed mode, `virtiofsd.log` contains `agentvm-composed-fs` output.

The tiny guest config share remains in v1 and carries `composed-binds.json`.

## Host Behavior

Composed mode generates the host backend manifest and guest bind manifest from
the same mount table before QEMU starts. Startup fails before QEMU when a
required source is missing, when a required writable source is inaccessible, or
when generated mount paths conflict.

The wrapper starts `agentvm-composed-fs` using:

```text
--manifest .sandbox/docker-vm/run/composed-fs-manifest.json
--socket-path .sandbox/docker-vm/run/virtiofs.sock
--tag <artifact manifest vm.virtiofs_tag>
```

During development, the binary is resolved from
`composed-fs/target/debug/agentvm-composed-fs` unless
`SANDBOX_WRAP_COMPOSED_FS_BIN` or `$PATH` provides one.

## Guest Behavior

Guest init mounts the tiny config share first. If
`/run/agentvm-config/composed-binds.json` exists, it enters composed mode:

- mount the composed export at the manifest `composed_mountpoint`
- bind each manifest entry into its final guest target
- create parent directories when requested
- create file placeholders for file binds
- fail boot for required bind failures
- log and skip optional bind failures

If `composed-binds.json` is absent, guest init uses the old project mount plus
`shares.txt` per-share reconstruction path.

## Validation Status

This ticket wires the gated q35 path and performs syntax/build-level checks.
Full boot, Docker readiness, payload readiness, bind-mount behavior, and
fallback validation are owned by `wra-oz9h`.
