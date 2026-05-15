---
id: wra-fqw7
status: closed
deps: [wra-vjat]
links: []
created: 2026-05-15T19:47:03Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-ia1a
tags: [validation, docker, vm, filesystem]
---
# Validate Docker on persistent root overlay

Validate whether Docker can use /var/lib/docker on the persistent root overlay. The desired model is that Docker writes to normal guest /var/lib/docker within the root overlay, with no separate Docker-only disk. However, if Docker overlay2 or storage behavior does not work correctly on the root overlay, we may need a second sparse Docker disk as a technical workaround.

Scenario should exercise Docker across relaunch: pull or load image, run container, create container/volume/layer state, shut down, relaunch same project, confirm Docker daemon starts and the image/container/volume state persists and is usable. Also check relevant filesystem features such as xattrs, d_type, whiteouts, and overlay nesting errors if Docker fails.

## Design

Add a live Docker persistence scenario after root overlay implementation. Capture dockerd logs, docker info, storage driver, mount output, and kernel errors. The test should decide with evidence whether root-overlay-only is viable. Do not add a separate Docker disk unless this validation exposes a concrete failure.

## Acceptance Criteria

Either Docker works and persists on the root overlay, with validation coverage; or a concrete failure is documented with logs and a follow-up/implementation ticket for a separate Docker disk workaround. The decision is recorded on the epic.


## Notes

**2026-05-15T20:09:55Z**

Validated Docker on persistent root overlay with ./vm-frontend/validate.sh live-docker after adding a final sync to the self-test payload and starting from a fresh .sandbox/docker-vm/state.raw. The three live Docker scenarios passed using the root overlay state disk: container egress allow pulled and ran alpine:3.22; no-net denial relaunch reused Docker state and ran cached alpine successfully while denying container egress; host-to-container published port scenario also ran from the same root overlay state and passed. Earlier exec format errors appeared when relaunching after Docker writes without an explicit sync before hard QEMU termination; adding sync before payload-ok resolved the observed issue. Decision: Docker works on root overlay for current validation, so no separate Docker disk is justified.
