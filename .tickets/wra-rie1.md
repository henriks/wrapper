---
id: wra-rie1
status: open
deps: []
links: []
created: 2026-05-16T15:50:47Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-piqm
tags: [frontend, launch, stability]
---
# Clean up frontend helper services on partial startup failure

Problem:
start_frontend_with_policy spawns composed-fs, config-fs, and vmnet helper threads before QEMU. If a later wait or QEMU spawn fails, the function returns before constructing RunningFrontend, so Drop cleanup is never reached for already-spawned helpers.

Relevant code:
- vm-frontend/src/launch.rs:245-291 spawns helper threads with a shutdown token but no owner guard.
- vm-frontend/src/launch.rs:260, 275, 291 wait for sockets after each spawn.
- vm-frontend/src/launch.rs:302-311 spawns QEMU after helpers are live.
- vm-frontend/src/launch.rs:206-227 only RunningFrontend::Drop performs cleanup after successful construction.

Impact:
Partial startup failure can leave background services, stale sockets, and confusing behavior on repeated launches in the same process.

Recommended fix:
Introduce a startup guard/supervisor that owns helper join handles and a shutdown token before each spawn. On error after the first helper starts, set shutdown, close/listener-stop helpers if supported, and join or otherwise prove termination. Move the guard into RunningFrontend after successful QEMU spawn.

Validation:
- Offline test with invalid QEMU path after helper startup asserting cleanup state and no lingering observable runtime sockets/threads.
- Live smoke variant that fails QEMU spawn after service readiness and can relaunch cleanly.

