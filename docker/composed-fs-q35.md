# q35 Composed Filesystem Integration

Ticket: `wra-0bcu`

Date: 2026-05-13

## Switch

The q35 composed filesystem path remains available explicitly for comparison
and troubleshooting:

```sh
sandbox-wrap --docker --docker-machine q35 ...
```

There is no longer a q35 per-share fallback path. All Docker VM modes use the
composed filesystem export plus the tiny readonly config share.

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

If `composed-binds.json` is absent, guest init fails boot because the old
`shares.txt` per-share reconstruction path has been removed.

## Validation Status

`wra-oz9h` validated the gated q35 path on 2026-05-13 after rebuilding the
appliance artifacts with the updated guest init.

Preflight checks:

- `python3 -m py_compile sandbox-wrap`
- `sh -n docker/guest-init.sh`
- `cargo build --manifest-path composed-fs/Cargo.toml --offline`
- `cargo test --manifest-path composed-fs/Cargo.toml --offline`
- host manifest and guest bind manifest generation probe

Integration checks:

- `--docker --docker-machine q35 --no-net` boots, reaches payload readiness,
  preserves the project working directory, exposes Docker through the socket
  proxy, and writes project files back to the host.
- Composed mode mounts one primary `virtiofs` export at `/run/agentvm-host`;
  guest paths such as the project directory, tool state, Docker state, and
  `/workspace` are reconstructed as bind mounts from that export.
- `--ro` entries reject writes with `Read-only file system`; `--rw` entries
  accept guest writes and persist them on the host.
- Docker bind mounts from `$PWD` work from inside the guest.
- `--docker-publish 28081:18081` forwards host localhost traffic to a server
  inside the guest and returned `publish-ok`.
Validation found one implementation bug: `agentvm-composed-fs` returned from
`main` immediately after starting the vhost-user daemon. The backend now calls
`daemon.wait()` after `daemon.start(listener)` so the process remains alive for
the guest session.

Remaining validation belongs to later rollout tickets:

- broader Codex/Copilot auth-state smoke coverage
- startup/readiness timing against the old q35 path before removal
- microvm launch and microvm plus composed-fs integration
