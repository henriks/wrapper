---
id: wra-n0xc
status: closed
deps: [wra-dz2y]
links: []
created: 2026-05-17T06:03:06Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [guest, rust, appliance]
---
# Plan and implement opt-in Rust guest-service appliance packaging

Follow-up from wra-lcbk. After the Rust guest-service crate has local parity tests, add an opt-in appliance packaging path. Relevant files: docker/build-appliance.sh installs guest assets and writes source_inputs in docker/out/artifact-manifest.json; docker/guest-init.sh starts guest-socket-bridge.py and guest-payload-server.py. Keep Python services as the default/fallback. Add source freshness coverage for the Rust binary and any source inputs that affect it. This is appliance-sensitive: pause before changes and require rebuild plus live validation.

## Acceptance Criteria

Appliance can be built with the Rust guest-service binary installed in opt-in mode; artifact manifest/source freshness tracks the binary/build inputs; Python default path remains unchanged; ./vm-frontend/validate.sh required plus live-payload/live-docker pass for the opt-in mode before any default switch.


## Notes

**2026-05-17T06:20:33Z**

Implemented offline opt-in packaging support. docker/build-appliance.sh now accepts AGENTVM_GUEST_SERVICE_BIN pointing to an executable inside the repo, installs it as /usr/local/libexec/agentvm-guest-service, and records its hash in source_inputs; Python guest services remain the default and guest-init startup is unchanged. Added docker/tests/test_build_appliance.sh for optional binary source_inputs hash coverage and missing-binary rejection; validate.sh guest-services runs it. Updated docker/guest-service-rust-spike.md and validation docs. Focused validation passed: bash -n docker/build-appliance.sh, docker/tests/test_build_appliance.sh, ./vm-frontend/validate.sh guest-services/docs/fmt/fast. Full live/required validation is currently blocked until appliance rebuild because docker/build-appliance.sh changed; live-smoke freshness check reports stale build-appliance hash. Need rebuild (optionally with AGENTVM_GUEST_SERVICE_BIN=guest-service/target/release/agentvm-guest-service after building it) before closing.

**2026-05-17T07:05:24Z**

After user rebuild, the default appliance freshness is fixed and validation passed on the Python-default artifact: live-payload passed (log /tmp/pi-bash-7a0ce20c1a9399c6.log), live-docker passed, and ./vm-frontend/validate.sh required passed in 101s (log /tmp/wra-n0xc-required-default-20260517-100330.log). However the current artifact was not built with AGENTVM_GUEST_SERVICE_BIN: docker/out/artifact-manifest.json has no guest-service/target/release/agentvm-guest-service source_input, and debugfs did not find /usr/local/libexec/agentvm-guest-service in docker/out/rootfs.raw. wra-n0xc remains open until an opt-in rebuild installs/tracks the binary and live/required validation passes for that artifact.

**2026-05-17T07:22:19Z**

Opt-in appliance rebuild verified. docker/out/artifact-manifest.json records guest-service/target/release/agentvm-guest-service sha256 c477e56..., source hash freshness check passed, and debugfs confirmed /usr/local/libexec/agentvm-guest-service exists in docker/out/rootfs.raw. Live validation passed on the opt-in artifact while Python remains default: live-payload passed (log /tmp/pi-bash-6d8767d1a30beef6.log), live-docker passed, and ./vm-frontend/validate.sh required passed in 96s (log /tmp/wra-n0xc-required-optin-20260517-102036.log). Closing wra-n0xc.
