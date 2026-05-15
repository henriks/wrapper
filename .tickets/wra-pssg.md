---
id: wra-pssg
status: closed
deps: [wra-gx6d]
links: []
created: 2026-05-15T10:37:17Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-neci
---
# Gate live validation on fresh appliance source hashes

Live validation currently trusts docker/out artifacts. We hit a real failure because docker/guest-payload-server.py was newer than rootfs.raw, so the live VM did not contain current guest code. Existing freshness checks are partial and tied to smoke hook behavior.

## Design

Extend docker/build-appliance.sh to record source hashes or mtimes for appliance inputs such as docker/guest-init.sh, docker/guest-payload-server.py, docker/guest-socket-bridge.py, appliance.env, and relevant build scripts in artifact-manifest.json. Teach self-test/live validation to compare current source state against the manifest before boot. Include clear diagnostics telling the user to rerun sudo ./docker/build-appliance.sh when stale.

## Acceptance Criteria

Changing guest-init.sh or either guest Python service makes live validation fail before QEMU boot with a stale appliance diagnostic. Rebuilding updates the manifest and allows validation to proceed. Unit tests cover freshness pass/fail cases.


## Notes

**2026-05-15T10:48:49Z**

Started after wra-gx6d. Initial code survey: docker/build-appliance.sh installs guest-init.sh, guest-socket-bridge.py, guest-payload-server.py, and writes artifact-manifest.json, but the manifest currently records only artifact paths/vm/guest/versions, not source hashes. vm-frontend/src/main.rs has ensure_smoke_hook_artifact_fresh(), a narrow mtime check for docker/guest-init.sh vs docker/out/rootfs.raw that only runs for --guest-http-smoke-url. This ticket should replace/generalize that with manifest source hash validation before self-test/live boot, with stale diagnostics telling users to rerun sudo ./docker/build-appliance.sh.

**2026-05-15T10:55:08Z**

Implemented source-hash freshness gating. docker/build-appliance.sh now records source_inputs hashes for appliance.env, build-appliance.sh, guest-init.sh, guest-payload-server.py, guest-socket-bridge.py, and refresh-pins.sh. self-test and guest HTTP smoke validation now call ensure_appliance_sources_fresh before boot; missing source hashes or changed source hashes emit a stale appliance diagnostic instructing rerun sudo ./docker/build-appliance.sh. Unit tests cover fresh pass, changed guest-init/payload-server/socket-bridge fail, and missing manifest entries fail. Validation run: docs/fmt/fast/fuzz-check passed. Host-live now fails before QEMU boot because current docker/out/artifact-manifest.json lacks source_inputs; cannot rebuild in this environment because sudo requires a password, so ticket remains open until appliance is rebuilt and host-live/required passes.

**2026-05-15T11:08:47Z**

docker/guest-init.sh changed while adding source-only testability for wra-otbe. This reinforces the need to rebuild with sudo ./docker/build-appliance.sh before live validation; current host-live still fails pre-boot with missing source_inputs in docker/out/artifact-manifest.json.

**2026-05-15T11:18:52Z**

docker/guest-payload-server.py changed again for payload frame limits/error reporting, so appliance artifacts definitely need sudo ./docker/build-appliance.sh before live/required validation can pass the source hash gate.

**2026-05-15T11:30:00Z**

After user rebuilt appliance artifacts, ./vm-frontend/validate.sh required passed end-to-end. Source hash freshness gate allowed live validation to proceed; host-live reached self-test: ok. Evidence: docs/fmt/offline tests/guest-services/fuzz-check and live self-test passed on 2026-05-15.
