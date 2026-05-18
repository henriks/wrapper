---
id: wra-vd8g
status: closed
deps: []
links: [wra-y335, wra-662v]
created: 2026-05-18T04:36:30Z
type: bug
priority: 2
assignee: Henrik Saksela
tags: [appliance, guest-service, rust, validation]
---
# Reject non-guest-compatible Rust guest-service binaries in appliance build

Rust opt-in appliance live-payload failed after building with AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/debug/agentvm-guest-service. The manifest contained agentvm_payload_service=rust and rootfs.raw contained /usr/local/libexec/agentvm-guest-service, but guest-init logged '/usr/local/libexec/agentvm-guest-service: not found'. Host inspection showed target/debug/agentvm-guest-service is dynamically linked for GNU/glibc with interpreter /lib64/ld-linux-x86-64.so.2, which is absent in the Alpine/musl guest. Building --target x86_64-unknown-linux-musl produces a static-pie binary suitable for the guest. docker/build-appliance.sh currently only checks that AGENTVM_GUEST_SERVICE_BIN is executable, not that it is guest-compatible.

## Acceptance Criteria

Rust opt-in appliance build or validation rejects a GNU/glibc-linked guest-service binary for the Alpine/musl guest, or documents/automates the correct musl/static build path. Tests cover the rejection/documentation so future rebuild instructions cannot accidentally use target/debug/agentvm-guest-service.


## Notes

**2026-05-18T05:39:43Z**

Linked to cleanup epic wra-9m5h through wra-o75s. This remains a prerequisite for deleting the Python guest-service path because the surviving Rust binary must be rejected early if it is not guest-compatible for appliance builds.

**2026-05-18T08:15:50Z**

Implemented appliance-side compatibility gate for AGENTVM_PAYLOAD_SERVICE=rust: docker/build-appliance.sh now requires readelf, rejects non-ELF scripts and non-musl dynamic interpreters such as /lib64/ld-linux-x86-64.so.2, and points users at cargo build --target x86_64-unknown-linux-musl --bin agentvm-guest-service. Updated docker/tests/test_build_appliance.sh with fake readelf coverage for glibc rejection and musl acceptance, and updated docker/guest-service-rust-spike.md with the musl opt-in build command. Validation: bash docker/tests/test_build_appliance.sh passed; ./vm-frontend/validate.sh guest-services passed; ./vm-frontend/validate.sh docs passed; ./vm-frontend/validate.sh required failed at live-smoke before exercising the changed appliance because docker/out artifacts are stale after docker/build-appliance.sh changed and sudo ./docker/build-appliance.sh cannot run in this non-interactive harness (sudo requires a password/TTY). User/privileged runner must rebuild appliance and rerun required before closing.

**2026-05-18T08:17:54Z**

Iteration 23 hardening: added docker/tests/test_build_appliance.sh coverage that AGENTVM_PAYLOAD_SERVICE=rust rejects a non-ELF executable script, not just a glibc-linked ELF. Targeted validation passed: bash docker/tests/test_build_appliance.sh; ./vm-frontend/validate.sh guest-services; git diff --check. Also built the real host guest-service with cargo build --bin agentvm-guest-service --offline and verified the appliance gate rejects target/debug/agentvm-guest-service with unsupported interpreter /lib64/ld-linux-x86-64.so.2. Built the documented musl target with cargo build --bin agentvm-guest-service --target x86_64-unknown-linux-musl --offline and verified the appliance gate accepts target/x86_64-unknown-linux-musl/debug/agentvm-guest-service. Required validation remains blocked because sudo -n true reports a password is required, so stale docker/out artifacts cannot be rebuilt in this harness.

**2026-05-18T08:33:40Z**

Required validation passed after host/operator appliance rebuild: ./vm-frontend/validate.sh required exited successfully, including offline tests, guest-services/build-appliance tests, fuzz target compilation, live-smoke with launch-config-ok, and live-setup-tools. Full output: /tmp/pi-bash-4593beabebdd1968.log. Closing wra-vd8g.
