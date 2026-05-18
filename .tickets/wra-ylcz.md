---
id: wra-ylcz
status: closed
deps: []
links: [wra-y335, wra-662v]
created: 2026-05-18T04:33:14Z
type: bug
priority: 2
assignee: Henrik Saksela
tags: [validation, appliance, root-overlay]
---
# Reset root overlay state for live-payload validation after appliance rebuild

After rebuilding the appliance with AGENTVM_PAYLOAD_SERVICE=rust, ./vm-frontend/validate.sh live-payload failed even though docker/out/rootfs.raw contains /usr/local/libexec/agentvm-guest-service and the manifest has agentvm_payload_service=rust. Console log showed agentvm-init starting rust payload server, then /usr/local/libexec/agentvm-guest-service: not found. The validation tier live-payload does not remove .sandbox/docker-vm/state.raw before boot, unlike live-smoke and live-docker, so stale persistent root overlay state can mask newly added lower-rootfs files after appliance rebuilds. Repro context: run Rust opt-in rebuild, then live-payload with an old .sandbox/docker-vm/state.raw. Suggested fix: make live-payload reset or isolate the root overlay state disk like live-smoke/live-docker, or make self-test run dirs use scenario-local state disks.

## Acceptance Criteria

After appliance rebuilds that add rootfs files, live-payload starts from fresh/isolate root overlay state and cannot fail because stale .sandbox/docker-vm/state.raw hides new lower-rootfs files. Add validation/script coverage for this behavior.


## Notes

**2026-05-18T04:36:30Z**

Follow-up correction: resetting .sandbox/docker-vm/state.raw and rerunning live-payload produced the same guest error. rootfs.raw contains /usr/local/libexec/agentvm-guest-service; the actual current root cause is that target/debug/agentvm-guest-service is a host GNU/glibc binary requesting /lib64/ld-linux-x86-64.so.2, which is absent in the Alpine/musl appliance. The stale root-overlay hypothesis is not proven by this failure; keep this ticket only if reproduced independently or re-scope it as validation isolation hardening.
