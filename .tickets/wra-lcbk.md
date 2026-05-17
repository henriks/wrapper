---
id: wra-lcbk
status: closed
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


## Notes

**2026-05-16T19:42:36Z**

Epic wra-bjaa vmnet/payload host-side async-boundary children are now closed. This guest-service Rust/Tokio spike remains intentionally unstarted because it is blocked by wra-g0uv, wra-sum7, and wra-dky9 and is appliance-sensitive: starting it likely touches docker guest assets/packaging and may require an appliance rebuild. Do not begin until those prerequisite service-bound tickets define the Python semantics to preserve and the user confirms appliance-sensitive work is acceptable.

**2026-05-16T21:43:09Z**

wra-dky9 is closed after appliance rebuild, live-docker, and required validation. Remaining prerequisite before guest-service Rust/Tokio spike is wra-sum7 (payload server idle/slow-writer bounds), which is appliance-sensitive and should define payload service bounds before the spike.

**2026-05-16T21:57:40Z**

Prerequisite status after Ralph iteration 10: wra-g0uv and wra-dky9 are closed. wra-sum7 remains blocked by wra-38ai until docker/guest-payload-server.py is rebuilt into the appliance and live/required validation passes. Do not start the Rust/Tokio guest-service spike until wra-sum7 closes; the spike remains appliance-sensitive.

**2026-05-17T06:00:20Z**

wra-sum7 and wra-38ai are now closed after rebuilt-appliance required validation passed. wra-lcbk is ready. Pausing before starting because this spike is appliance-sensitive and may add a Rust/Tokio guest-service binary and packaging/appliance changes.

**2026-05-17T06:03:32Z**

Spike completed without adding a guest binary or changing appliance startup. Added docker/guest-service-rust-spike.md capturing the bounded Python baseline, Rust service semantics to preserve, packaging/source freshness risks, Tokio/offline dependency constraint, and staged opt-in migration plan. Created follow-ups: wra-dz2y (repo-local Rust guest-service protocol crate skeleton), wra-n0xc (opt-in appliance packaging, depends on wra-dz2y), and wra-y335 (parity/live validation before any default switch, depends on wra-n0xc). Ran ./vm-frontend/validate.sh docs; latest full required gate passed immediately before this doc-only spike continuation after appliance rebuild.
