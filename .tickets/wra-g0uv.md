---
id: wra-g0uv
status: open
deps: []
links: [wra-lcbk, wra-dky9, wra-sum7]
created: 2026-05-16T15:50:47Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-piqm
tags: [guest, docker, readiness]
---
# Gate payload readiness on guest service readiness

Problem:
Guest readiness is effectively payload-server readiness, not service readiness. guest-init launches dockerd, starts the socket bridge and payload server immediately, and the host waits for payload ping. Docker may still be initializing when the first user payload runs.

Relevant code:
- docker/guest-init.sh:360-366 starts dockerd.
- docker/guest-init.sh:368-381 starts socket bridge and payload server immediately after dockerd spawn.
- vm-frontend/src/launch.rs:302-312 marks launch running after QEMU spawn.
- vm-frontend/src/main.rs waits for payload readiness before running user payload.
- docker/runtime-contract.md:254 describes readiness expectations.

Impact:
First-command Docker usage can fail or hang, especially on cold root overlay boots.

Recommended fix:
Gate payload-server readiness on guest health: required mounts present, Docker socket exists, and Docker _ping succeeds. Only report running/ready after the payload path and Docker are ready when Docker is in scope.

Validation:
- Cold-boot live test where first payload command is docker version/docker info with no sleeps, repeated several times.
- Unit or integration coverage for state/readiness transition semantics.

