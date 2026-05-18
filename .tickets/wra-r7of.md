---
id: wra-r7of
status: closed
deps: []
links: []
created: 2026-05-16T15:50:48Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [frontend, guest, concurrency]
---
# Allocate per-launch payload control ports to avoid cross-project collisions

Problem:
Internal payload control defaults to a fixed host port, so concurrent projects on the same host can collide even though project runtimes are expected to be isolated. Validation also uses fixed published ports.

Relevant code:
- vm-frontend/src/main.rs:3516 and 3524 define/default payload control port behavior.
- docker/runtime-contract.md:288 documents runtime port behavior.
- vm-frontend/validate.sh:160 and nearby live smoke commands use fixed ports.

Impact:
Two different projects launched concurrently can fail due to 127.0.0.1 port conflicts. Validation can be flaky on busy hosts.

Recommended fix:
Allocate and reserve an ephemeral loopback port for internal payload control and plumb the selected port into policy. Keep fixed ports only for explicit user-published ports.

Validation:
- Integration/live test launching two projects concurrently.
- Update validation helpers to allocate free ports for smoke scenarios.


## Notes

**2026-05-18T10:37:57Z**

Follow-up cleanup scan under wra-emj5 confirmed this ticket remains the owner for fixed payload control ports. The implementation should delete DEFAULT_HOST_PAYLOAD_PORT/ensure_payload_listener fixed-port behavior for internal payload control, use typed PortPair parsing consistently, and reserve/hold ephemeral loopback ports rather than hiding the old constant behind another option.

**2026-05-18T12:57:08Z**

Implemented per-launch ephemeral payload-control host port allocation for implicit listeners, keeps explicit --host-payload-listener ports unchanged, holds a reservation until the vmnet service is about to bind host listeners, and changed validation published-port defaults to ephemeral helper ports. Live validation blocked by stale appliance artifacts requiring sudo ./docker/build-appliance.sh in this environment.

**2026-05-18T12:57:36Z**

Validation: targeted payload listener/client tests pass; ./vm-frontend/validate.sh required passed offline tiers and used an ephemeral publish-payload-port before failing live-smoke on stale appliance artifacts. sudo ./docker/build-appliance.sh could not run in this environment because sudo requires a password/TTY, so live concurrent-project validation remains pending on the host after rebuilding the appliance.

**2026-05-18T12:58:01Z**

Added offline concurrent-reservation coverage: two implicit payload listeners allocate distinct loopback ports and both remain reserved until handed to launch.

**2026-05-18T13:03:38Z**

Post-rebuild validation: ./vm-frontend/validate.sh required passed. live-smoke used ephemeral publish-payload-port=52881; live-docker used ephemeral published container host port=34477. Offline concurrent-reservation coverage plus live required validation cover the per-launch ephemeral payload control change.
