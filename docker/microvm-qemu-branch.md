# Microvm QEMU Launch Branch

Ticket: `wra-ek03`

Date: 2026-05-13

## Switch

The microvm launch branch is now the Docker VM default:

```sh
sandbox-wrap --docker ...
```

The explicit form is also accepted:

```sh
sandbox-wrap --docker --docker-machine microvm ...
```

The old q35 per-share fallback remains available during the fallback window:

```sh
sandbox-wrap --docker --docker-legacy-per-share-fs ...
```

The microvm branch requires composed fs. This is deliberate: the migration goal
is one composed filesystem export plus the tiny boot config share, not a
duplicate microvm implementation of the old per-share export model.

## Command Shape

The branch follows the command shape validated in
`docker/microvm-spike.md`:

- `-machine microvm,acpi=off,memory-backend=mem,isa-serial=on`
- `-enable-kvm`
- `-cpu host`
- direct kernel and initrd boot
- `memory-backend-memfd` with `share=on`
- `virtio-blk-device` for rootfs and Docker data
- `virtio-rng-device`
- `vhost-user-fs-device` for the composed export and config share
- `-netdev user,...` plus `virtio-net-device`

The q35 branch still uses the existing PCI devices and `-nic` shorthand.

`docker/check-qemu-command-shape.py` checks both command shapes without
booting QEMU. It is intended as the lightweight regression check for this
branch until full VM validation is automated.

## Current Scope

This ticket only adds the gated command-construction branch and documents the
decision. Full boot validation belongs to `wra-11dm`.

Expected next validation:

- boot `--docker --docker-composed-fs --docker-machine microvm`
- verify Docker and payload readiness
- verify project path identity and host writeback
- verify Docker bind mounts from `$PWD`
- verify `--ro`, `--rw`, `--no-net`, and `--docker-publish`
- compare behavior against the validated q35 composed path

## Deviations From Spike

The spike used ordinary upstream `virtiofsd` for workspace and config exports
because q35 composed-fs validation had not completed yet. The implemented
branch uses the composed backend for the primary filesystem export and keeps
the tiny readonly config share. This matches the migration plan and keeps the
microvm device count below the documented limit.
