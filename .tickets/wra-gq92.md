---
id: wra-gq92
status: closed
deps: [wra-otbe]
links: []
created: 2026-05-15T10:38:58Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-neci
tags: [validation, payload, guest]
---
# Test payload protocol framing, limits, and cross-language compatibility

Add focused tests for the host payload client/server contract and the guest Python implementation. Relevant areas include vm-frontend self-test payload control code, docker/guest-payload-server.py, and any Rust request/response framing helpers used to publish commands into the guest. Cover fragmented stdio/socket frames, partial reads/writes, oversized requests and responses, invalid JSON/headers, timeout behavior, concurrent requests if supported, EOF mid-frame, and clear error reporting. Include a compatibility test that uses the real Python payload server against the Rust client without booting the full VM where possible.

## Design

Separate pure framing tests from subprocess compatibility tests. The compatibility test can run the Python script locally with temporary sockets/ports and assert the Rust side observes the same protocol semantics used in live validation. Bound all tests with short timeouts to catch hangs.

## Acceptance Criteria

Payload framing has offline tests for fragmentation, invalid data, size limits, EOF, and timeout cases. A Rust-to-Python compatibility test exercises the real guest payload server code outside QEMU. Live validation includes a large payload request/response scenario and reports protocol errors with enough context to diagnose guest vs host failures.


## Notes

**2026-05-15T11:06:47Z**

wra-otbe added a first real Python payload server compatibility-style unittest: it drives guest-payload-server.py over a socketpair with an R frame, observes O output and X exit-code frames, and validates ping/failure frames. wra-gq92 should build on docker/tests/test_guest_services.py for fragmentation, size limits, EOF mid-frame, timeout, and Rust-to-Python compatibility.

**2026-05-15T11:18:52Z**

Added first payload protocol hardening slice while wra-otbe remains open: Rust payload_client now enforces the same 16MiB frame payload limit on send/receive, rejects oversized receive before allocation, has EOF/truncation tests, proptest coverage for round-trip frames and arbitrary bounded bytes, and a Rust-to-real-Python guest-payload-server compatibility test over loopback. Offline docs/fmt/guest-services/fast/fuzz-check passed. Live large-payload matrix scenario remains pending wra-kh0g/live rebuild.

**2026-05-15T11:55:12Z**

Live payload stress attempt uncovered a hang instead of passing validation. ./vm-frontend/validate.sh live-payload booted and reached phase=running-payload, then the outer 900s timeout fired. Artifacts are in .sandbox/docker-vm/self-test-payload. state.json still said running for qemu_pid 3686320, but no matching QEMU process remained after timeout. vmnet-events.log shows payload control opened and host sent a large request (65536 bytes/44 guest frames plus 5020 bytes) with no guest response/exit frames afterward. Treat this as live acceptance blocker and investigate host-ingress/payload-control large request progress before closing.

**2026-05-15T11:57:57Z**

Implemented live payload stress scenario and fixed the resulting host-ingress backpressure bug. Added self-test --payload-stress and validate.sh live-payload, documented in AGENTS.md/README/validation-workflow. The first live-payload run hung after the host sent a 64KiB+ request; root cause was HostIngressBridge discarding bytes when smoltcp send_to_session accepted only a partial/zero write. Added pending_guest_write buffering/backpressure and regression test host_ingress::tests::large_host_payload_backpressures_instead_of_discarding_unsent_bytes. Validation: targeted self_test tests passed; host_ingress::tests:: passed (9 passed/1 ignored); ./vm-frontend/validate.sh live-payload passed with payload-stress-ok; ./vm-frontend/validate.sh required passed end-to-end after the fix.
