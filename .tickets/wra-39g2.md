---
id: wra-39g2
status: closed
deps: []
links: [wra-b9b1]
created: 2026-05-18T11:45:10Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-emj5
tags: [appliance, guest-service, validation]
---
# FOLLOW-UP: Detect stale guest-service binary during appliance build

While validating wra-b9b1 after an appliance rebuild, live-docker failed because the appliance installed target/x86_64-unknown-linux-musl/debug/agentvm-guest-service with sha256 c4e97e7b..., which predated the new docker-bridge subcommand. guest-init had been rebuilt to call "agentvm-guest-service docker-bridge", but the packaged binary still printed the old usage and rejected docker-bridge, so Docker TCP was unavailable. Running cargo build --target x86_64-unknown-linux-musl --bin agentvm-guest-service produced a new binary sha256 4891fad... with docker-bridge help output. build-appliance.sh currently validates ELF compatibility and records the binary hash, but it does not detect that the supplied binary is stale relative to guest-service source changes.

## Design

Add a freshness mechanism for AGENTVM_GUEST_SERVICE_BIN: either make the appliance build invoke the musl cargo build itself, or record/check enough source inputs (guest-service, payload-protocol, Cargo.toml/Cargo.lock as needed) to reject stale supplied binaries. Keep the solution simple and avoid adding compatibility paths.

## Acceptance Criteria

A rebuild after guest-service source changes cannot package an older guest-service binary without a clear error; artifact freshness/validation reflects the guest-service source-to-binary dependency; tests or validation coverage document the stale-binary scenario.


## Notes

**2026-05-18T11:52:32Z**

Fresh-binary rebuild resolved wra-b9b1 live validation: artifact manifest now records target/x86_64-unknown-linux-musl/debug/agentvm-guest-service sha256 4891fad..., live-docker and required passed. The bug remains valid as a hardening task because the previous appliance build accepted a stale but ELF-compatible guest-service binary and only live validation caught the missing docker-bridge subcommand.

**2026-05-18T12:31:41Z**

Starting implementation audit. This will touch docker/build-appliance.sh and likely appliance manifest source/hash behavior, so it is an appliance-input change and should be batched with the wra-p06p guest-init rebuild before live/required validation.

**2026-05-18T12:33:55Z**

Implemented stale guest-service binary guard. docker/build-appliance.sh now treats Cargo.toml, Cargo.lock, guest-service/Cargo.toml, guest-service/src/lib.rs, guest-service/src/main.rs, payload-protocol/Cargo.toml, and payload-protocol/src/lib.rs as guest-service source inputs, records them in artifact-manifest source_inputs, and rejects a repository AGENTVM_GUEST_SERVICE_BIN when any of those source files are newer than the binary. vm-frontend appliance freshness required inputs now include these guest-service/payload-protocol sources so normal launch/self-test detect stale appliance artifacts after source edits. Added docker/tests/test_build_appliance.sh coverage for manifest source recording and stale-binary rejection. Local validation passed: bash -n docker/build-appliance.sh docker/guest-init.sh docker/tests/test_build_appliance.sh; bash docker/tests/test_build_appliance.sh; cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --bin agentvm-frontend appliance_source_hashes; python3 -m unittest docker.tests.test_guest_services; ./vm-frontend/validate.sh guest-services; ./vm-frontend/validate.sh docs. Because docker/build-appliance.sh and required source hashes changed, Henrik must rerun sudo ./docker/build-appliance.sh before live/required validation and close.

**2026-05-18T12:36:08Z**

Additional local validation while awaiting appliance rebuild: ./vm-frontend/validate.sh fast && ./vm-frontend/validate.sh fuzz-check passed with timeout 300s. Closure remains blocked on sudo ./docker/build-appliance.sh and required/live validation because docker/build-appliance.sh and appliance source-input freshness requirements changed.

**2026-05-18T12:41:02Z**

Henrik rebuilt appliance on 2026-05-18. Verified docker/out/artifact-manifest.json now records the new source inputs (12 total), including Cargo.toml, Cargo.lock, guest-service/Cargo.toml, guest-service/src/lib.rs, guest-service/src/main.rs, payload-protocol/Cargo.toml, and payload-protocol/src/lib.rs. ./vm-frontend/validate.sh required passed with timeout 300s. Closed after validation.
