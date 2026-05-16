---
id: wra-ylfx
status: open
deps: []
links: []
created: 2026-05-16T15:50:47Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-piqm
tags: [frontend, guest, persistence]
---
# Add graceful guest shutdown before killing QEMU

Problem:
The frontend kills QEMU directly during termination paths. The wrapper attempts a guest sync in some payload paths, but sync failure is only a warning and self-test shutdown paths do not consistently sync first. Guest teardown also uses forced service kills and poweroff -f.

Relevant code:
- vm-frontend/src/launch.rs:174-183 kills QEMU in RunningFrontend::terminate.
- vm-frontend/src/main.rs:195 and payload completion paths call terminate after best-effort flush.
- docker/guest-init.sh:248-259 kills critical guest services and forces poweroff/reboot.

Impact:
Persistent root overlay, Docker metadata, and setup-tool installs can lose data or corrupt state if QEMU is hard-killed after dirty writes.

Recommended fix:
Implement graceful shutdown: diagnostic sync, guest poweroff or QMP system_powerdown, wait up to a documented timeout, then force kill. Make sync failure fatal for persistence/setup validation. Apply the same flush path to self-test.

Validation:
- Extend live-persistence to write many files plus Docker/setup markers, shutdown, relaunch, and assert no fsck/kernel errors and all markers remain.
- Run live-setup-tools and required validation before closing.

