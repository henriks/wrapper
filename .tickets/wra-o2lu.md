---
id: wra-o2lu
status: closed
deps: []
links: []
created: 2026-05-15T19:46:40Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-ia1a
tags: [vm, filesystem, persistence, design]
---
# Design guest root overlay boot and state disk layout

Design how the guest should boot with an immutable rootfs lower layer and a project-local persistent writable overlay state disk. Current rootfs is /dev/vda mounted read-only from docker/out/rootfs.raw. Current /dev/vdb is docker-data.raw mounted at /var/lib/docker. We need a new state disk role that backs overlay upper/work for the whole guest root.

Relevant files: docker/build-appliance.sh and artifact-manifest kernel cmdline/initramfs contents; docker/guest-init.sh current init; vm-frontend/src/lib.rs QEMU drive args/kernel_cmdline; vm-frontend/src/launch.rs disk creation; docker/README.md and OPERATIONS.md. Need determine whether overlay setup belongs in initramfs before switch_root or can be handled by the current init path. The current kernel cmdline uses root=/dev/vda rootfstype=ext4 ro init=/usr/local/sbin/agentvm-init quiet.

## Design

Document the exact boot sequence: mount immutable rootfs lower, mount persistent ext4 state disk, ensure upper/work dirs, mount overlay as new root, switch_root/pivot_root, then run agentvm-init from the overlay root. Include device naming assumptions, fsck/format behavior, failure diagnostics, and how composed-fs/config mounts are performed after overlay root is active. Decide names/paths: e.g. .sandbox/docker-vm/state.raw and guest mount staging paths. Account for reset behavior.

## Acceptance Criteria

A design note or ticket note specifies the root overlay boot sequence, disk file names, guest mount paths, kernel/initramfs changes, and failure modes. Follow-up implementation tickets have enough detail to start without rediscovery.


## Notes

**2026-05-15T19:49:54Z**

Design captured in docker/root-overlay-design.md. Key decisions: replace Docker-only docker-data.raw with project-local .sandbox/docker-vm/state.raw as a sparse ext4 overlay state disk; keep docker/out/rootfs.raw as immutable lower root; assemble overlay root before main guest init, preferably in initramfs; run /usr/local/sbin/agentvm-init after switch_root into the overlay; Docker uses /var/lib/docker on the root overlay by default; add a separate Docker disk only if live validation proves Docker-on-overlay fails. The doc also records current behavior, target host state layout, failure diagnostics, and validation requirements.
