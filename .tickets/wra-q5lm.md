---
id: wra-q5lm
status: open
deps: [wra-ieju]
links: []
created: 2026-05-15T19:43:27Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-bjlw
tags: [vm, filesystem, persistence, design]
---
# Reassess guest home and root persistence model

Clarify the guest persistence model so future shadow/home changes do not keep inheriting Bubblewrap-era assumptions. Current implementation has no persistent root overlay: docker/out/rootfs.raw is attached readonly as /dev/vda; .sandbox/docker-vm/docker-data.raw is formatted ext4 on first launch and mounted only at /var/lib/docker; docker/guest-init.sh mounts /home as tmpfs, then composed-fs currently maps <project>/.sandbox/home to the host-natural guest HOME path. This .sandbox/home special case appears to be legacy from the original Bubblewrap-based script and should be reconsidered separately from shadow backing.

Relevant code/docs: vm-frontend/src/runtime_manifest.rs RuntimeMount::persistent_home and guest_runtime_mounts; vm-frontend/src/main.rs runtime_mounts creates .sandbox/home and guest_payload_env sets HOME to host_home_dir(); docker/guest-init.sh mounts /home tmpfs; docker/OPERATIONS.md and requirements.md document .sandbox/home.

## Design

First document the actual current model and intended target: immutable rootfs plus Docker-only data disk versus a future persistent root overlay. Decide whether default guest HOME should be ephemeral unless explicitly mapped by config, or whether it should remain project-persistent through a generic configured share. If changing behavior, prefer generic config/share mechanisms over special RuntimeMount::persistent_home naming. This ticket is intentionally lower priority than shadow semantics to avoid expanding wra-u05c scope.

## Acceptance Criteria

There is a documented decision on whether .sandbox/home remains, moves to a generic .sandbox/root/home/<user> backing, becomes an explicit recipe/config share, or is removed in favor of a persistent root overlay. Follow-up implementation tickets are created if needed. Documentation no longer implies docker-data.raw is a general persistent overlay.

