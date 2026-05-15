# Persistent Root Overlay Design

This design replaced the earlier Docker-only persistence disk with the intended VM state model: an immutable appliance root filesystem plus a project-local writable overlay for the guest root.

## Previous State

- `docker/out/rootfs.raw` is an ext4 appliance image.
- QEMU attaches it read-only as the first virtio block device (`/dev/vda`).
- Kernel cmdline mounted it directly as root:
  `root=/dev/vda rootfstype=ext4 ro init=/usr/local/sbin/agentvm-init quiet`.
- The frontend creates `.sandbox/docker-vm/docker-data.raw`, formats it ext4, attaches it as `/dev/vdb`, and `docker/guest-init.sh` mounts it at `/var/lib/docker`.
- `docker/guest-init.sh` mounts `/home` as tmpfs, while composed-fs maps `<project>/.sandbox/home` onto the host-natural guest `$HOME` path.

This meant the only persistent guest paths were Docker's data root, configured/composed shares, and the special `.sandbox/home` mount. There was no persistent writable root overlay.

## Target State

- `rootfs.raw` remains immutable and read-only.
- A project-local sparse ext4 state disk, e.g. `.sandbox/docker-vm/state.raw`, backs overlayfs `upperdir` and `workdir`.
- The guest root is an overlay mount:
  - lower: immutable `/dev/vda` rootfs
  - upper: state disk `root/upper`
  - work: state disk `root/work`
- The guest runs normally from the overlay root. Writes under `/home`, `/usr/local`, `/var/lib/docker`, package caches, tool installs, and other ordinary guest paths persist across relaunch.
- Docker uses `/var/lib/docker` on the root overlay by default. A separate Docker disk is introduced only if live validation proves Docker cannot run correctly on the overlay root.
- `.sandbox/home` is not part of the target runtime model; `$HOME` is a normal guest path unless explicitly overlaid by configured shares.

## Boot Sequence

The overlay root should be assembled before the main guest init starts. The robust approach is an initramfs stage:

1. Mount devtmpfs so virtio block devices are available.
2. Load/ensure required modules: virtio block, ext4, overlay.
3. Mount `/dev/vda` read-only as the lower appliance root. If the kernel/initramfs has already mounted it as `/`, using the current root as the overlay lower is an acceptable implementation fallback.
4. Mount `/dev/vdb` as the persistent state disk.
5. Create state directories if missing:
   - `<state>/root/upper`
   - `<state>/root/work`
6. Mount overlay at a temporary new-root mountpoint with:
   - `lowerdir=<lower>`
   - `upperdir=<state>/root/upper`
   - `workdir=<state>/root/work`
7. `switch_root` into the overlay root and execute `/usr/local/sbin/agentvm-init`.
8. `agentvm-init` then performs the existing runtime setup: proc/sys/dev/pts/run/tmp mounts, composed-fs bind mounts, network setup, dockerd, socket bridge, and payload server.

The current `root=/dev/vda ... init=/usr/local/sbin/agentvm-init` flow mounts the lower root directly and starts too late conceptually. An implementation may either customize the generated Alpine initramfs or post-process it to install an AgentVM `/init` wrapper. The final boot path should not depend on mounting the state disk only at `/var/lib/docker`.

## Host State Layout

Preferred paths:

```text
.sandbox/docker-vm/
  lock
  state.raw              # persistent sparse ext4 state disk for root overlay
  run/                   # per-launch manifests, sockets, logs
```

If Docker requires a separate disk after validation:

```text
.sandbox/docker-vm/
  state.raw              # root overlay state
  docker.raw             # optional Docker-only workaround disk
```

Do not keep the Docker-only `docker-data.raw` model as the primary persistence mechanism.

## Failure Handling

Overlay setup failures should fail early with clear console diagnostics. Useful diagnostics include:

- missing block device (`/dev/vda` or `/dev/vdb`)
- ext4 mount failure for lower or state disk
- missing overlay module/support
- overlay mount failure including kernel error text where available
- switch_root failure

The host should continue to preserve `console.log`, QEMU logs, and state path information in launch artifacts.

## Validation Requirements

Root overlay validation:

1. Launch project VM and write marker files under ordinary guest paths such as `$HOME`, `/usr/local`, and an XDG cache/state path.
2. Shut down cleanly.
3. Relaunch with the same project state disk.
4. Assert marker files and metadata persist.
5. Assert `--reset` removes the overlay state.

Docker validation:

1. Start Docker on the overlay root.
2. Pull/load and run an image.
3. Create persistent Docker state such as an image, container, or volume.
4. Relaunch and confirm Docker starts and state remains usable.
5. Capture `docker info`, storage driver, mount output, dockerd logs, and kernel messages on failure.

Only if Docker fails on the root overlay with concrete evidence should a separate Docker disk be added.
