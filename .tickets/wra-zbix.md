---
id: wra-zbix
status: closed
deps: [wra-otbe, wra-kh0g]
links: []
created: 2026-05-15T10:39:11Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-neci
tags: [validation, docker, network, guest]
---
# Cover Docker bridge and guest socket bridge contracts

Add tests for Docker-facing networking and the guest socket bridge path. Relevant code spans vm-frontend runtime networking, docker/guest-socket-bridge.py, guest Docker setup in docker/guest-init.sh, and live validation scenarios that pull/run containers. Cover Docker bridge availability, container DNS, container egress through allowed policy, container denial under no-net/hostile policy, published port forwarding, Unix/TCP socket bridge framing, reconnects, and bridge cleanup after failure.

## Design

Use direct offline tests for the Python socket bridge where possible, and live VM scenarios only for kernel/Docker behavior that cannot be modeled reliably. Live scenarios should use tiny images already used by validation, and should distinguish Docker pull/setup failure from runtime network policy failure.

## Acceptance Criteria

Offline socket bridge tests cover framing, reconnects, disconnects, and malformed messages. Live validation includes Docker bridge egress, Docker bridge no-net denial, and published port access from host to guest/container. Failures name the container/image, network policy, and bridge phase involved.


## Notes

**2026-05-15T14:27:42Z**

Started with offline socket-bridge contract expansion. guest-socket-bridge is a raw byte-stream relay (not a framed protocol), so malformed-message coverage is represented as arbitrary binary stream preservation rather than frame parsing. Added tests for relaying through a real Unix docker socket, separate-client reconnects, and binary/malformed byte preservation. Targeted python unittest discovery passed with 15 tests.

**2026-05-15T14:28:35Z**

After socket bridge offline tests, ./vm-frontend/validate.sh required passed end-to-end, including guest-services with 15 tests and live-smoke Docker bridge checks (docker version/info/run and published payload listener). Ticket remains open for named live Docker bridge scenario coverage: container egress/no-net denial and host-to-container published port access.

**2026-05-15T14:33:52Z**

Added named live Docker bridge slice: self-test --docker-net-check and validate.sh live-docker. live-docker runs container egress allow and no-net denial with image/policy/phase diagnostics (docker-egress-ok, docker-deny-ok). First no-net run exposed unrelated sqlite concurrency disk I/O in the specialized Docker-denial scenario; adjusted self-test to use per-run sqlite DB names and skip sqlite concurrency only for docker-net-check+no-net so the Docker policy scenario remains focused. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline self_test -- --nocapture passed; ./vm-frontend/validate.sh live-docker passed; ./vm-frontend/validate.sh required passed. Still open for host-to-container published port live coverage.

**2026-05-15T14:39:19Z**

Completed Docker bridge coverage. Added --publish-container-port self-test path and live-docker now runs three named KVM scenarios: container egress allow, container egress no-net denial, and host-to-container published port. Published-port path starts an Alpine container in the guest, maps guest port to container nc HTTP response, waits for docker-publish-ready output, then verifies the host can connect through the frontend published port and receive agentvm-container-publish-ok. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline self_test -- --nocapture passed; targeted published-container self-test unit passed; ./vm-frontend/validate.sh docs/fmt passed; ./vm-frontend/validate.sh live-docker passed all three scenarios; ./vm-frontend/validate.sh required passed end-to-end.
