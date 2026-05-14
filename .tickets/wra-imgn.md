---
id: wra-imgn
status: closed
deps: [wra-fi7l, wra-cetz]
links: []
created: 2026-05-14T20:13:17Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-f16x
tags: [tests, e2e, sandbox, network, filesystem]
---
# Add hostile guest end-to-end sandbox smoke tests

Add a slow/ignored end-to-end hostile guest smoke profile that exercises the actual sandbox product guarantees rather than only unit-level helpers. This should validate that the assembled wrapper/VM configuration blocks the obvious breakout and bypass attempts from inside the guest. Relevant code includes vm-frontend/src/main.rs self-test payload construction, launch/runtime manifest generation, composed-fs manifests, vmnet runtime/gateway, and docker/guest-init.sh.

## Design

Create an ignored test or documented self-test mode that boots/simulates the guest enough to attempt: writes to readonly/config mounts; symlink/path escape attempts from workspace; access to metadata IP 169.254.169.254; loopback/private/link-local TCP egress; UDP/443/QUIC-style bypass; malformed DNS and denied domain DNS; allowed control listeners such as payload/docker bridges still work when configured. Keep it separate from normal cargo test because it may require artifacts/QEMU/container support.

## Acceptance Criteria

- Slow test is ignored by default and documents the exact command/environment needed to run it.
- Test attempts both filesystem and network bypasses from the guest perspective.
- Expected blocked operations fail closed with observable diagnostics/logs where available.
- Positive controls remain: allowed workspace write and configured host ingress/control listener still work.


## Notes

**2026-05-14T20:32:39Z**

Added explicit self-test --hostile profile. The hostile payload adds guest-side probes for config private key readability, workspace symlink escape to /run/agentvm-config/mitm-ca.key, metadata IP TCP connect denial, loopback TCP connect denial, and DNS denial when --no-net is active, while preserving existing positive controls for workspace writes, tool state, Docker, and payload control. Added an ignored hostile_guest_self_test_profile test with an exact AGENTVM_HOSTILE_SELF_TEST_RUN=1 cargo test command; the ignored test runs self-test --hostile --no-net only when explicitly enabled. Verification: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed (128 lib tests + 22 main tests run, 6 ignored total across frontend).
