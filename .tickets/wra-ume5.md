---
id: wra-ume5
status: closed
deps: [wra-o2lu]
links: []
created: 2026-05-15T19:46:48Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-ia1a
tags: [vm, filesystem, persistence]
---
# Implement persistent root overlay state disk

Implement the root overlay state disk from the design ticket. Replace the current Docker-only disk role with a project-local sparse ext4 state disk that backs overlayfs upper/work for the guest root. The immutable appliance rootfs remains read-only.

Current code context: RuntimePaths::data_disk currently points to docker-data.raw; launch.rs ensure_data_disk formats it; FrontendConfig::qemu_args attaches rootfs readonly and dockerdata as the second virtio-blk; guest-init.sh mounts /dev/vdb at /var/lib/docker. This ticket should rename/reframe the host-side path and QEMU drive as root/state overlay storage, and update guest boot scripts/initramfs as designed.

## Design

Prefer removing/renaming Docker-specific terms rather than preserving compatibility names. Create/format the state disk on first launch, attach it as a virtio block device, and make guest boot mount it as overlay upper/work for /. Ensure state dirs are created with correct modes. Preserve useful launch diagnostics when overlay setup fails. Do not keep a separate Docker mount in this ticket unless Docker validation proves it necessary.

## Acceptance Criteria

Guest root is writable through overlayfs while the lower rootfs remains immutable. Files written to ordinary guest paths persist across VM relaunch for the same project. /var/lib/docker is not specially mounted from docker-data.raw in the normal path. Unit/model tests cover command shape/path naming where practical; live validation ticket proves behavior end-to-end.


## Notes

**2026-05-15T19:50:17Z**

Started implementation orientation. Code touchpoints found: vm-frontend/src/lib.rs RuntimePaths::data_disk currently points to root_dir/docker-data.raw and QEMU attaches it as drive id dockerdata; vm-frontend/src/launch.rs ensure_data_disk formats that disk before launch; docker/guest-init.sh mounts /dev/vdb at /var/lib/docker; docker/build-appliance.sh manifest still names docker_data_device/docker_data_mount and kernel cmdline mounts rootfs directly with init=/usr/local/sbin/agentvm-init. Implementation should rename/reframe this disk as state.raw/root overlay state and move overlay assembly before main guest init, likely via initramfs customization per docker/root-overlay-design.md.

**2026-05-15T19:53:21Z**

Partial implementation progress: renamed frontend runtime disk role from Docker data to root overlay state. RuntimePaths now uses state_disk at .sandbox/docker-vm/state.raw; QEMU attaches it as drive id statedisk; launch.rs creates/formats/validates VM state disk instead of Docker data disk; build-appliance manifest now names state_disk_device and state_disk_role=root-overlay. guest-init.sh now performs an early root overlay bootstrap before normal init: mounts devtmpfs and tmpfs /run, waits for /dev/vda and /dev/vdb, mounts /dev/vda ro as lower, mounts /dev/vdb as state, creates root/upper and root/work, mounts overlay, then chroots into the overlay root and re-execs agentvm-init with AGENTVM_ROOT_OVERLAY_READY=1. Removed the guest mount of /dev/vdb at /var/lib/docker. Targeted checks passed: cargo fmt for vm-frontend, ./docker/tests/test_guest_init.sh, and cargo test --manifest-path vm-frontend/Cargo.toml --offline builds_microvm_stream_command_without_usernet_or_hostfwd.

**2026-05-15T19:54:29Z**

Broader offline validation after partial implementation: ./vm-frontend/validate.sh fast passed. This includes composed-fs offline tests, vm-frontend offline tests, and tui terminal tests in this tier. Note: full required/live validation still cannot pass until appliance artifacts are rebuilt from changed docker/guest-init.sh/build-appliance.sh source hashes and the overlay boot path is exercised live.

**2026-05-15T19:56:22Z**

Added regression assertion that default runtime state disk path is project-local .sandbox/docker-vm/state.raw and absolute. Targeted validation passed: cargo fmt and cargo test --manifest-path vm-frontend/Cargo.toml --offline frontend_defaults_runtime_under_absolute_project -- --nocapture.

**2026-05-15T19:57:42Z**

Refined early overlay bootstrap: if mounting /dev/vda as a separate lower mount fails because the kernel/initramfs already has it mounted as /, guest-init now falls back to a non-recursive bind mount of the current root as the overlay lower. Added a guest-init shell test proving setup_root_overlay is a no-op after AGENTVM_ROOT_OVERLAY_READY=1, so the chroot/re-exec path will not recurse. Updated docker/root-overlay-design.md to mention the current-root lower fallback. Targeted validation: ./docker/tests/test_guest_init.sh passed.

**2026-05-15T19:59:34Z**

Added state disk path to failure/diagnostic artifacts: frontend_artifact_summary now includes state_disk=... and launch state JSON includes state_disk. Tests updated to assert state.raw appears in artifact summary and launch state. Targeted validation passed: cargo fmt; cargo test --manifest-path vm-frontend/Cargo.toml --offline frontend_artifact_summary_names_key_run_artifacts -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --lib writes_launch_state_snapshot -- --nocapture.

**2026-05-15T20:05:12Z**

Live-persistence attempt after user rebuilt appliance reached guest-init overlay setup but timed out waiting for payload readiness. console.log showed agentvm-init setting up persistent root overlay, then repeated cat: cannot open /proc/cmdline before services started. Root cause: chrooting into the overlay root lost the initramfs /proc mount, while guest-init reads /proc/cmdline before its normal proc mount step. Fix applied: setup_root_overlay now mounts proc inside /run/agentvm-newroot/proc before chroot/re-exec. Because docker/guest-init.sh changed again, appliance artifacts must be rebuilt before the next live validation attempt. Targeted checks after fix: ./docker/tests/test_guest_init.sh and cargo fmt --check passed.

**2026-05-15T20:07:59Z**

Root overlay implementation now validated live for ordinary guest root persistence. After user rebuilt the appliance, live-persistence first exposed two issues: missing /proc inside overlay chroot (fixed by mounting proc before re-exec) and using /usr/local/share for a non-root payload persistence marker (fixed by using /var/tmp/agentvm-root-persistence and sync). With a fresh state.raw, ./vm-frontend/validate.sh live-persistence passed: first run wrote /var/tmp/agentvm-root-persistence/marker on the root overlay; second run with the same state.raw verified the marker persisted. This proves state.raw root overlay works for ordinary guest writable paths across relaunch. Docker-on-overlay remains separate wra-fqw7.
