---
id: wra-nsc9
status: closed
deps: [wra-iyae]
links: []
created: 2026-03-27T21:22:04Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-hggq
tags: [docker, vm, image]
---
# Create build pipeline for the Docker appliance VM image

Add the build assets needed to produce the immutable guest appliance described in docker.md.

## Design

Scope:
- minimal Debian 12 guest build via debootstrap
- pinned Docker Engine packages
- tiny init/PID 1 boot flow
- guest-side bridge from vsock to /var/run/docker.sock
- read-only root image output plus kernel/initrd or kernel image needed for Cloud Hypervisor

The output must match the runtime contract and be consumable by the host-side VM manager.

## Acceptance Criteria

The repo contains a reproducible build path for the appliance artifacts.

The output format matches the runtime contract.

The appliance boots to a state where Docker can become reachable through the guest bridge.


## Notes

**2026-03-27T21:31:58Z**

Added the appliance build assets under docker/: build-appliance.sh, appliance.env, guest-init.sh, guest-vsock-bridge.py, README.md, and the tracked out/ manifest location. The build output contract is docker/out/{rootfs.raw,vmlinuz,initrd.img,artifact-manifest.json}. Verification gap: I syntax-checked the scripts and confirmed the entrypoint fails fast without root, but I did not run a privileged networked build or boot the guest because that requires root plus real Docker package version pins in docker/appliance.env.

**2026-03-27T21:36:39Z**

Added docker/refresh-pins.sh so version resolution is automated but still explicit. The script fetches Docker's Packages.gz for the configured suite and arch, resolves exact versions for docker-ce, docker-ce-cli, containerd.io, docker-buildx-plugin, and docker-compose-plugin, and rewrites only the corresponding pin lines in docker/appliance.env.

**2026-03-27T21:50:38Z**

Pivoted the appliance build from Debian to Alpine to target a genuinely smaller guest. The new build uses Alpine minirootfs plus docker-engine, linux-virt, mkinitfs, python3, e2fsprogs, iproute2, and util-linux. Removed buildx/compose and the full Debian package flow. Rootfs logical size default is now 512M.

**2026-03-27T21:53:19Z**

Verified the Alpine appliance build completed successfully. Generated artifacts: docker/out/rootfs.raw at 512M logical size, docker/out/initrd.img at 6.2M, docker/out/vmlinuz at 12M, and docker/out/artifact-manifest.json with Alpine version metadata. The unpacked rootfs is about 258M, a substantial reduction from the earlier Debian-based attempt.

**2026-03-31T20:40:00Z**

Live boot testing on a newer Cloud Hypervisor reached the guest and exposed an appliance bug: the guest kernel successfully mounted the root filesystem, but `switch_root` failed to execute `/usr/local/sbin/agentvm-init` with `ENOENT`. The cause was the script's `#!/bin/bash` shebang in an Alpine guest that does not install Bash. Updated `docker/guest-init.sh` to use POSIX `/bin/sh`, switched `source` to `.`, and replaced `wait -n` with a shell-portable polling loop. Rebuilding the appliance is required for this fix to take effect because the script is baked into both `rootfs.raw` and `initrd.img`.

**2026-03-31T20:47:00Z**

Follow-up live boot testing reached `agentvm-init` and exposed a second appliance bug: the guest services were redirected to `/var/log/*.log`, but the root filesystem is mounted read-only by design. `containerd`, `dockerd`, and the vsock bridge therefore failed immediately with `Read-only file system`, triggering guest shutdown and making the host-side Docker readiness probe time out. Updated `docker/guest-init.sh` to redirect those service logs into `/run/*.log` instead. This also requires rebuilding the appliance artifacts.

**2026-03-31T20:53:00Z**

Further live boot testing showed the guest now reaches `agentvm-init`, starts `containerd`, `dockerd`, and the vsock bridge, but one of those services still exits immediately and triggers guest shutdown before the host-side Docker readiness probe can succeed. Added failure-time log dumping in `docker/guest-init.sh` so `/run/containerd.log`, `/run/dockerd.log`, and `/run/vsock-bridge.log` are echoed to the guest console before teardown. This is a diagnostics improvement only, and also requires rebuilding the appliance artifacts to take effect.

**2026-03-31T20:58:00Z**

The dumped guest service logs showed the standalone `containerd` invocation was the immediate failure path: it tried to create `/var/lib/containerd` on the read-only root filesystem and exited, while `dockerd` was already launching and managing its own containerd instance successfully under writable paths. Simplified `docker/guest-init.sh` to stop starting a separate `containerd` process and to supervise only `dockerd` plus the vsock bridge. This requires another appliance rebuild to take effect.

**2026-03-31T21:04:00Z**

Subsequent live guest logs showed `dockerd` still exiting immediately with `failed to start daemon: Devices cgroup isn't mounted`. Updated `docker/guest-init.sh` to create and mount `/sys/fs/cgroup` as cgroup v2 before starting Docker. This is another guest image fix and requires rebuilding the appliance artifacts to take effect.

**2026-03-31T21:15:00Z**

Additional live debugging after guest log mirroring showed `dockerd` now reaches `API listen on /var/run/docker.sock`, while the guest vsock bridge only logs `listening on vsock port 1075` and never accepts a client. The Alpine guest image already contains the needed virtio-vsock kernel modules (`vsock`, `vmw_vsock_virtio_transport_common`, and `vmw_vsock_virtio_transport`), but the appliance was relying on autoloading. Updated `docker/guest-init.sh` to load those modules explicitly before starting the vsock bridge. This also requires rebuilding the appliance artifacts to take effect.

**2026-03-31T21:35:00Z**

Adjusted guest workspace mounting to better support Docker bind mounts from inside the sandbox. `agentvm-init` now reads a per-project `agentvm_project=<host_path>` kernel cmdline argument, mounts the virtio-fs share at that original absolute project path inside the guest when it is under `/home/...`, and bind-mounts the same share to `/workspace` as a compatibility alias. This allows guest bind-mount source paths to match the absolute project path seen by the sandboxed Docker client. Because the mount logic lives in `docker/guest-init.sh`, this change requires rebuilding the appliance artifacts.
