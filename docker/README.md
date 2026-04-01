# Docker Appliance Build

This directory contains the build assets for the immutable Docker appliance VM
used by `sandbox-wrap --docker`.

The current build target is a genuinely smaller Alpine-based guest rather than
the earlier Debian-based appliance. The goal is to keep the VM small and quick
to build while still running a normal Docker daemon inside the guest.

For runtime behavior, reset semantics, and troubleshooting, see
`docker/OPERATIONS.md`.

## Outputs

Run:

```bash
docker/refresh-pins.sh
sudo docker/build-appliance.sh
```

The build writes immutable appliance artifacts to `docker/out/`:

- `rootfs.raw`: ext4 root filesystem image, intended to be mounted read-only
- `vmlinuz`: guest kernel copied from the built rootfs
- `initrd.img`: guest initramfs copied from the built rootfs
- `artifact-manifest.json`: launcher-facing metadata

The launcher should consume `artifact-manifest.json` instead of hardcoding
artifact paths or guest parameters.

## Build Strategy

The build downloads Alpine `minirootfs`, configures `apk` repositories for the
selected Alpine branch, and installs only the packages needed for the guest:

- `docker-engine`
- `linux-virt`
- `mkinitfs`
- `python3`
- `e2fsprogs`
- `iproute2`
- `util-linux`

This intentionally excludes guest-side Docker CLI plugins like buildx and
compose.

## Prerequisites

- Linux host
- root privileges for `docker/build-appliance.sh`
- `curl`
- `tar`
- `truncate`
- `mkfs.ext4`
- `chroot`

## Version Pins

Refresh the Alpine package version pins in `docker/appliance.env` before
building:

```bash
docker/refresh-pins.sh
```

This keeps version selection explicit and source-controlled while still
automating the lookup against Alpine's official package indexes.

## Guest Layout

The guest boots with `init=/usr/local/sbin/agentvm-init`. That init:

- mounts the Docker data disk at `/var/lib/docker`
- mounts the `virtio-fs` workspace share at the original absolute project path
  seen by the sandboxed Docker client and keeps `/workspace` as a compatibility
  alias
- configures the guest NIC for QEMU user-mode networking
- starts `dockerd`
- starts `docker/guest-socket-bridge.py` to forward a guest TCP port to
  `/var/run/docker.sock`

Persistent Docker state belongs only on the separate sparse Docker data disk.
The rootfs stays read-only at runtime.
