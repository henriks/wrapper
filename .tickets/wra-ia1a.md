---
id: wra-ia1a
status: closed
deps: []
links: []
created: 2026-05-15T19:46:31Z
type: epic
priority: 0
assignee: Henrik Saksela
tags: [vm, filesystem, persistence, docker]
---
# Replace Docker-only data disk with persistent VM root overlay

Implement the intended VM persistence model: immutable appliance rootfs plus a project-local persistent writable overlay for the guest root filesystem. The current implementation creates .sandbox/docker-vm/docker-data.raw, formats it ext4 on first launch, attaches it as /dev/vdb, and guest-init mounts it only at /var/lib/docker. That is not the desired model. Docker should normally use the same persistent root overlay as everything else inside the VM. A separate Docker data disk is acceptable only if live validation proves Docker does not work correctly on the root overlay.

Current code/docs context: vm-frontend/src/launch.rs ensure_data_disk creates docker-data.raw; vm-frontend/src/lib.rs RuntimePaths::data_disk and qemu args attach it as dockerdata; docker/guest-init.sh mounts /dev/vdb at /var/lib/docker and separately mounts /home as tmpfs; runtime_manifest.rs maps .sandbox/home as a special persistent-home mount; docker/README.md, docker/OPERATIONS.md, docker/runtime-contract.md, and requirements.md document Docker-only data persistence and .sandbox/home.

Desired model: docker/out/rootfs.raw remains immutable/read-only lower rootfs. A project-local sparse ext4 state disk backs overlayfs upper/work directories for the guest root. Guest writes to /home, /usr/local, /var/lib/docker, package/tool installs, caches, etc. persist through the overlay. Explicit host shares still overlay/mount specific paths from composed-fs. .sandbox/home should no longer be needed as a special persistence mechanism once root overlay is in place.

## Design

Proceed in evidence-driven steps: design the boot/initramfs/guest-init changes for overlay root, implement the state disk and boot flow, validate ordinary persistence and Docker persistence across relaunches, then remove Docker-only disk and .sandbox/home special cases if validation passes. If Docker-on-overlay fails for concrete filesystem reasons, add a second sparse Docker disk as a documented technical workaround rather than treating Docker persistence as the primary state model.

## Acceptance Criteria

A VM launched for a project has a persistent writable root overlay backed by project-local sparse storage. Writes under normal guest paths, including home/tool install paths, persist across relaunch. Docker works and persists across relaunch either on the root overlay or, if proven necessary, on a separate sparse Docker disk. Docs accurately describe immutable lower rootfs plus persistent overlay state. Required validation ./vm-frontend/validate.sh required passes, and live persistence/Docker scenarios are covered before closing.


## Notes

**2026-05-15T20:27:54Z**

Final validation: ./vm-frontend/validate.sh required passed after fixing self-test isolation. The first required attempt failed in live-smoke due a stale/corrupt .agentvm-self-test-sqlite DB causing guest sqlite disk I/O errors; added reset_sqlite_concurrency_db to remove the per-run sqlite DB/WAL/SHM before each self-test and a unit regression test. Final required gate passed docs drift, rustfmt, composed-fs offline tests, vm-frontend offline tests, offline guest service tests, fuzz target compilation, and live-smoke from a fresh state.raw. Earlier live-persistence and live-docker also passed from fresh state disks.
