---
id: wra-o75s
status: closed
deps: [wra-vd8g, wra-y335]
links: [wra-662v, wra-2ku5]
created: 2026-05-18T05:37:14Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-9m5h
tags: [cleanup, guest-service, rust]
---
# Delete Python guest service path after Rust parity decision

The Rust guest service work and wra-y335 parity validation should end with one guest service implementation, not a permanent Python/Rust split. Once parity, appliance compatibility, and Docker socket bridge ownership are settled, switch the Rust service to the default and delete the Python guest service path and associated bridge adapters that no longer have a product purpose.

## Design

Depends on wra-y335 and wra-vd8g. Use wra-y335 to decide whether the Docker socket bridge belongs in Rust guest-service scope or remains a separate host/guest component. After that decision, remove the losing implementation path, update appliance packaging, validation scripts, and docs. Avoid compatibility shims except for config file compatibility if any field semantics change.

## Acceptance Criteria

There is one default guest service path; obsolete Python service files, appliance hooks, and tests are removed or explicitly retained only as validation fixtures; Docker socket bridge ownership is documented in the surviving architecture; live setup-tool and Docker validation tiers pass or record environment limitations before close.


## Notes

**2026-05-18T07:21:43Z**

From wra-2ku5: payload service implementation selection is no longer encoded as agentvm_payload_service on the kernel cmdline. build-appliance writes PAYLOAD_SERVICE into /etc/agentvm.env, and guest-init defaults to python from that env. When this ticket deletes the Python guest service path after Rust parity, remove the PAYLOAD_SERVICE switch/env default and start the Rust service directly rather than preserving a compatibility selector.

**2026-05-18T08:11:47Z**

Iteration 21 cleanup reflection: still blocked by wra-vd8g and wra-y335. Cleanup direction remains to delete the Python guest-service path and PAYLOAD_SERVICE selector after Rust guest-service compatibility/parity is validated; do not add a long-term Python/Rust compatibility switch.

**2026-05-18T08:16:07Z**

Prerequisite wra-vd8g has source-level implementation and offline coverage for rejecting non-guest-compatible Rust service binaries, but cannot close in this harness because required validation needs a privileged appliance rebuild after docker/build-appliance.sh changed. Keep wra-o75s blocked until wra-vd8g is closed and wra-y335 parity live validation passes.

**2026-05-18T08:35:01Z**

Iteration 31 update: wra-vd8g is closed, so this deletion ticket is now blocked only by wra-y335. The cleanup direction remains to delete the Python guest-service path and temporary PAYLOAD_SERVICE selector once Rust parity validation passes; do not keep both implementations as a compatibility layer. wra-y335 currently needs a privileged opt-in Rust appliance rebuild using the musl binary path before live-payload/live-docker/required validation can run.

**2026-05-18T08:39:08Z**

Iteration 32 start: wra-y335 is closed after Rust opt-in appliance passed live-payload, live-docker, and required validation. Cleanup target is now unblocked. Direction: make Rust guest payload service the sole payload server path, delete the Python payload-server artifact and PAYLOAD_SERVICE selection/fallback, and keep Docker socket bridge separate unless a dedicated bridge replacement ticket says otherwise. Do not keep the Python payload server as a compatibility layer.

**2026-05-18T08:39:31Z**

Iteration 32 audit after start: rg found payload-service selection/removal targets in docker/build-appliance.sh (PAYLOAD_SERVICE_IMPL, AGENTVM_PAYLOAD_SERVICE validation, Python payload-server install, optional Rust install, manifest inputs), docker/guest-init.sh (PAYLOAD_SERVICE default/selector, payload_server_command branches), docker/tests/test_build_appliance.sh and docker/tests/test_guest_init.sh, docs, and frontend appliance source hash inputs. Python guest-socket-bridge remains separate Docker bridge support and should not be deleted by this payload-server cleanup.

**2026-05-18T08:48:22Z**

Iteration 33 implementation progress: deleted docker/guest-payload-server.py and removed the PAYLOAD_SERVICE/Python-vs-Rust selector. docker/build-appliance.sh now requires AGENTVM_GUEST_SERVICE_BIN, validates it as an Alpine/musl-compatible ELF, installs it as /usr/local/libexec/agentvm-guest-service, records it in source_inputs, and no longer installs the Python payload server or writes PAYLOAD_SERVICE to /etc/agentvm.env. docker/guest-init.sh now starts the Rust guest service directly. Offline guest-service Python unittest coverage now covers only the remaining Docker socket bridge; Rust payload service coverage remains in guest-service crate tests and live validation. Updated source freshness required inputs and docs to remove the Python payload-server path. Validation passed: bash docker/tests/test_build_appliance.sh; ./docker/tests/test_guest_init.sh; python3 -W error::ResourceWarning -m unittest discover -s docker/tests -p '*test*.py' -v; cargo test --manifest-path vm-frontend/Cargo.toml --offline --features validation-self-test payload_client::tests:: -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --features validation-self-test appliance_source_hashes -- --nocapture; ./vm-frontend/validate.sh guest-services; ./vm-frontend/validate.sh docs; ./vm-frontend/validate.sh fmt; ./vm-frontend/validate.sh fast (log /tmp/pi-bash-9815e552dbd06a39.log); git diff --check. Required validation is pending because appliance artifacts are stale after deleting/changing guest appliance sources, and this harness still cannot run sudo -n true (password required).

**2026-05-18T08:49:18Z**

Iteration 33 follow-up cleanup: renamed the remaining guest-init payload path variable to GUEST_SERVICE_PATH after removing the Python payload server. Reran ./docker/tests/test_guest_init.sh and ./vm-frontend/validate.sh guest-services (log /tmp/iter33-guest-services.log); git diff --check still passes. Remaining PAYLOAD_SERVICE rg hit is only the negative assertion in docker/tests/test_build_appliance.sh.

**2026-05-18T08:52:44Z**

Iteration 33 final validation after host/operator appliance rebuild: manifest source_inputs no longer include docker/guest-payload-server.py and include target/x86_64-unknown-linux-musl/debug/agentvm-guest-service. Live console logs show 'agentvm-init: starting rust payload server'. Passed ./vm-frontend/validate.sh live-payload (log /tmp/pi-bash-194b1950de12ad76.log), ./vm-frontend/validate.sh live-docker, and ./vm-frontend/validate.sh required (log /tmp/pi-bash-b9c98fefde7854fc.log). Closing wra-o75s.
