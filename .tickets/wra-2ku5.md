---
id: wra-2ku5
status: closed
deps: []
links: [wra-o75s]
created: 2026-05-18T05:37:00Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-9m5h
tags: [cleanup, guest, config]
---
# Replace dynamic kernel cmdline metadata with structured guest config

Guest launch metadata appears to be spread through kernel command-line construction and parsing. Kernel cmdline should be limited to boot-critical flags, not used as an extensible application protocol for dynamic paths, payload URLs, or service metadata. Move non-boot metadata into a structured guest config delivered through the existing guest-visible channel, then delete duplicated cmdline parsing/building glue.

## Design

Audit launch-time kernel cmdline construction, guest-side parsing, payload service metadata, and appliance defaults. Keep .sandbox/config.json compatibility rules intact, but do not preserve internal kernel-cmdline fields for compatibility. Use a small typed config file or manifest format with tests for serialization/parsing and guest consumption.

## Acceptance Criteria

Non-boot launch metadata is no longer encoded in kernel cmdline; guest-side consumers read structured config; obsolete cmdline fields and parsing helpers are deleted; config-json.md is updated only if .sandbox/config.json semantics change; required validation is recorded before close.


## Notes

**2026-05-18T07:09:54Z**

Started in Ralph iteration 6. Audit found dynamic metadata currently appended in FrontendConfig::kernel_cmdline (project path, guest IP/gateway/prefix/DNS/MAC, HTTP smoke URL) and parsed by docker/guest-init.sh get_cmdline_value before mounting agentvm-config. Plan: write a typed launch metadata JSON file into runtime guest-config and expose it through the existing agentvm-config virtiofs; keep artifact kernel_cmdline for boot-critical flags only. Also remove build-appliance agentvm_payload_service cmdline use by writing payload service selection to /etc/agentvm.env.

**2026-05-18T07:14:46Z**

Implemented first pass: runtime manifest generation now writes guest-config/launch.json (schema_version 1, project_path, network assignment, optional http_smoke_url) and exposes it on the agentvm-config virtiofs. FrontendConfig no longer appends dynamic agentvm_* metadata to the kernel cmdline. guest-init now loads launch.json after mounting config virtiofs; get_cmdline_value and agentvm_* cmdline parsing are deleted. Payload service selection moved from agentvm_payload_service kernel arg to /etc/agentvm.env via build-appliance. Updated guest-init/build-appliance tests and validation docs. Targeted validation passed: ./vm-frontend/validate.sh guest-services; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features; cargo test --manifest-path vm-frontend/Cargo.toml --offline --features validation-self-test. Logs include /tmp/pi-bash-707f9cd91668c986.log for validation-self-test cargo tests. Live/required validation is pending because current docker/out appliance artifacts are root-owned and stale after docker/build-appliance.sh and docker/guest-init.sh changes; sudo requires a password in this harness, so artifacts must be rebuilt on the host with sudo ./docker/build-appliance.sh before ./vm-frontend/validate.sh required can pass.

**2026-05-18T07:16:47Z**

Iteration 7 validation update: docs/fmt/fuzz-check passed via ./vm-frontend/validate.sh docs && ./vm-frontend/validate.sh fmt && ./vm-frontend/validate.sh fuzz-check. ./vm-frontend/validate.sh live-smoke correctly failed before boot with stale appliance artifacts (docker/build-appliance.sh hash mismatch) and requested sudo ./docker/build-appliance.sh. sudo -n is unavailable in this harness, so required validation remains blocked on host-side artifact rebuild.

**2026-05-18T07:19:25Z**

Iteration 8 hardening: added prepare-level regression coverage in parses_frontend_prepare_defaults_and_policy to run prepare_frontend_launch_with_policy, assert qemu cmdline contains no dynamic agentvm_* metadata, and assert guest-config/launch.json contains project/network/http_smoke_url metadata. Validation passed: targeted cargo test --manifest-path vm-frontend/Cargo.toml --offline --features validation-self-test parses_frontend_prepare_defaults_and_policy; ./vm-frontend/validate.sh fast passed (full output /tmp/pi-bash-4e34b245d08cda1c.log). Required/live validation still pending host appliance rebuild because docker/out artifacts are stale and sudo is interactive in this harness.

**2026-05-18T07:20:44Z**

Iteration 9 hardening: live self-test payload now asserts /run/agentvm-config/launch.json exists, validates schema/project/network fields against AGENTVM_SELF_TEST_PROJECT, and asserts /proc/cmdline no longer carries agentvm_project=. This makes required live validation check the new guest-consumed launch config once appliance artifacts are rebuilt. Validation passed: targeted cargo test --manifest-path vm-frontend/Cargo.toml --offline --features validation-self-test self_test_payload_covers_workspace_docker_and_bind_mount -- --nocapture; ./vm-frontend/validate.sh guest-services; git diff --check.

**2026-05-18T07:21:57Z**

Iteration 10 docs/status: updated vm-frontend/README.md to use validation-only agentvm-self-test, document guest-config/launch.json in self-test/failure artifacts, and explain that --guest-http-smoke-url is delivered through launch.json rather than kernel cmdline. Validation passed: ./vm-frontend/validate.sh docs; ./vm-frontend/validate.sh fmt; git diff --check. Also noted to wra-o75s that future Python guest-service deletion should remove the PAYLOAD_SERVICE selector rather than preserving it.

**2026-05-18T07:28:39Z**

User rebuilt appliance artifacts, then ./vm-frontend/validate.sh required was run. First run exposed validation bug wra-80tf: live-setup-tools executed stale vm-frontend/target/debug/agentvm and missed launch.json. Fixed wra-80tf, reran ./vm-frontend/validate.sh required successfully. Live-smoke output included self-test: launch-config-ok, confirming guest consumed /run/agentvm-config/launch.json and no dynamic cmdline fallback was needed. Full passing validation log: /tmp/pi-bash-2be9ae6dec1fa6b4.log.
