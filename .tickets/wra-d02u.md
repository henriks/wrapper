---
id: wra-d02u
status: closed
deps: []
links: [wra-0djh]
created: 2026-05-18T12:44:48Z
type: task
priority: 3
assignee: Henrik Saksela
parent: wra-piqm
tags: [cleanup, guest, docker, boot]
---
# FOLLOW-UP: Pin guest Docker storage driver and reduce unsupported snapshotter probe noise

wra-0djh live guest boot audit on 2026-05-18 found dockerd/containerd repeatedly logging expected-but-noisy unsupported storage probes: overlay2 fails because Docker's data root is on the AgentVM root overlay (kernel logs: 'overlay: filesystem on /var/lib/docker/check-overlayfs-support.../upper not supported as upperdir'; dockerd logs: failed to mount overlay storage-driver=overlay2), fuse-overlayfs is not installed, and containerd skips btrfs/devmapper/erofs/zfs snapshotters/differs. Docker still works in validation, but the logs make real boot/storage errors harder to spot and may add startup work. Investigate setting an explicit supported storage driver/config for the appliance path (likely vfs unless a better state-disk layout is introduced) and disabling/quieting unsupported containerd snapshotter probes without adding compatibility paths.

## Acceptance Criteria

A normal live boot shows Docker still passes live-docker/required validation; dockerd/containerd logs no longer report avoidable overlay2/fuse-overlayfs unsupported-driver errors, or the ticket records why the probe noise must be retained. Any docker/guest-init.sh or appliance config change is treated as an appliance-input change requiring sudo ./docker/build-appliance.sh before live validation.


## Notes

**2026-05-18T12:57:08Z**

Implemented guest Docker storage-driver pinning to vfs via appliance env and dockerd --storage-driver, with offline guest-init/build-appliance tests updated. Live validation is still blocked until appliance artifacts are rebuilt with sudo ./docker/build-appliance.sh; current environment cannot provide sudo password.

**2026-05-18T12:57:36Z**

Validation: ./docker/tests/test_guest_init.sh, ./docker/tests/test_build_appliance.sh, vm-frontend docs check, and targeted vm-frontend cargo tests pass. ./vm-frontend/validate.sh required passed offline tiers and then failed at live-smoke because docker/out/artifact-manifest.json is stale after appliance-input edits; sudo ./docker/build-appliance.sh cannot run here because sudo requires a password/TTY. Containerd snapshotter enumeration may still be emitted by Docker-managed containerd; this change deliberately pins dockerd's graphdriver to vfs to remove the avoidable overlay2/fuse storage-driver failure path without adding a separate containerd supervision/config path.

**2026-05-18T13:03:38Z**

Post-rebuild validation: ./vm-frontend/validate.sh required passed, and ./vm-frontend/validate.sh live-docker passed all allow/deny/published-port scenarios. Current guest-dockerd.log entries after rebuild show Docker daemon starting with storage-driver=vfs and '[graphdriver] trying configured driver: vfs'; the avoidable overlay2/fuse-overlayfs graphdriver errors are gone. Docker-managed containerd still logs informational unsupported snapshotter/differ skip probes (btrfs/devmapper/erofs/zfs); retaining those for now rather than adding a separate containerd config path.
