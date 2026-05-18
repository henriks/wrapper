# Docker Appliance Build

This directory contains the build assets for the immutable appliance VM used by
the Rust VM-only frontend.

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
- `docker-cli`
- `linux-virt`
- `mkinitfs`
- `python3`
- `e2fsprogs`
- `iproute2`
- `util-linux`
- `bash`
- `nodejs`
- `npm`
- pinned upstream `mise`

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

The guest boots with an immutable lower rootfs and project-local persistent root
state disk. `agentvm-init` assembles the writable root overlay and then:

- mounts the Rust composed-fs workspace/config shares at the configured original
  absolute project path
- configures the guest NIC for the Rust userspace vmnet gateway
- starts `dockerd`
- starts `/usr/local/libexec/agentvm-guest-service docker-bridge` to forward a
  guest TCP port to `/var/run/docker.sock`
- starts `/usr/local/libexec/agentvm-guest-service` so the host frontend can
  launch the requested payload inside the guest

Persistent guest state, including Docker's `/var/lib/docker`, lives on the
sparse root overlay state disk. The appliance rootfs remains the immutable lower
layer at runtime.
