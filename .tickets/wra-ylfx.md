---
id: wra-ylfx
status: open
deps: [wra-6qa2]
links: [wra-8fjd, wra-m7gg, wra-d6vo, wra-35eb, wra-2qij, wra-6qa2]
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


## Notes

**2026-05-16T16:59:13Z**

Cross-ticket note from wra-2qij: self-test now performs bounded diagnostic sync before force terminate and fails if sync/deadline fails. Normal launch still warns on sync failure, and RunningFrontend::terminate still force-kills QEMU. True graceful guest/QMP poweroff remains for wra-ylfx rather than being hidden in the async-service-IO slice.

**2026-05-16T21:46:50Z**

Review after wra-35eb/wra-m7gg/wra-2qij: still valid. The payload side now has a local-abort path and bounded diagnostic sync is used for self-test, but normal launch still treats flush_guest_filesystems failure as a warning and RunningFrontend::terminate still force-kills QEMU. This ticket remains the owner for guest/QMP poweroff, bounded graceful wait, fatal persistence/setup sync semantics, and live persistence/setup validation.

**2026-05-18T10:37:51Z**

Follow-up cleanup epic wra-emj5 adds wra-6qa2 as a prerequisite. Implement graceful shutdown by replacing the force-first QEMU lifecycle with one consolidated lifecycle where guest/QMP shutdown is primary and force kill is a timeout fallback. Do not add graceful shutdown as a parallel adapter around the current wrapper stack.

**2026-05-18T10:52:55Z**

Cleanup prerequisite wra-6qa2 has collapsed the supervised QEMU wrapper family into one SupervisedQemuLifecycle path in vm-frontend/src/launch.rs. Graceful guest/QMP shutdown should be added in that single lifecycle path before the existing forced termination fallback, rather than by reintroducing with_* wrapper variants.
