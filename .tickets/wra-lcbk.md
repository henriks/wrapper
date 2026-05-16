---
id: wra-lcbk
status: open
deps: [wra-g0uv, wra-sum7, wra-dky9]
links: [wra-dky9, wra-sum7, wra-g0uv]
created: 2026-05-16T16:13:56Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-bjaa
tags: [async, tokio, guest, payload, docker, appliance]
---
# Spike Rust/Tokio guest-service binary after Python service bounds are fixed

Problem:
The guest currently runs Python long-lived services for payload control and Docker socket bridging. A Rust/Tokio replacement may consolidate them into one bounded guest service, but this is a packaging/build-system change and should happen only after current semantics and bounds are clear.

Grounding:
- docker/build-appliance.sh:94-102 installs guest assets, including Python socket bridge and payload server.
- docker/build-appliance.sh:153 installs python3 in the appliance.
- docker/build-appliance.sh:182 and source manifest tracking include the Python scripts.
- docker/guest-payload-server.py implements the payload frame protocol and PTY/process-group behavior.
- docker/guest-socket-bridge.py implements the Docker TCP-to-Unix bridge.
- vm-frontend/src/payload_client.rs:572 and related tests expect the current frame protocol.

Recommended direction:
Do not immediately replace Python with asyncio or Rust. First land/service-spec the bounds and readiness tickets. Then prototype a single agentvm-guest-service in Rust/Tokio that preserves the exact payload frame protocol and Docker bridge behavior, with bounded tasks, cancellation, process-group cleanup, PTY handling, and metrics/log summaries. Keep Python services as fallback during the spike.

Relationships:
- Depends on or should follow wra-g0uv, wra-sum7, and wra-dky9 so the Rust version ports known semantics rather than unknown bugs.
- Related to wra-r7of only indirectly; per-launch host ports reduce collision pressure but do not solve guest-service resource bounds.

Risks:
Rust packaging increases appliance build complexity, source freshness checks, cross/musl concerns, and recovery risk if the binary fails early. PTY/session behavior is subtle and must remain compatible.

Validation:
- Keep Python compatibility tests until replacement is complete.
- Add Rust guest-service protocol tests mirroring docker/tests/test_guest_services.py and vm-frontend payload client tests.
- Add appliance manifest/source freshness coverage for the new binary.
- Run required validation plus live-docker and live-payload before switching default.

