---
id: wra-b1a5
status: in_progress
deps: []
links: []
created: 2026-05-18T19:52:06Z
type: bug
priority: 0
assignee: Henrik Saksela
tags: [security, workspace, composed-fs]
---
# BUG: .sandbox is visible inside guest workspace

The project workspace mount exposes the host project directory wholesale, so the guest can see and potentially read/write project-local agentvm state under .sandbox (config.json, docker-vm/run artifacts, state paths, logs, CA material locations, setup-tool files). This was observed from /home/hsaksela/Code/planb and is also implied by RuntimeMount::workspace(project) mounting the project root directly. This should not be papered over by hiding more files under .sandbox; .sandbox should be excluded or shadowed from the guest workspace by the composed-fs layer/runtime manifest.

## Acceptance Criteria

Guest workspace lookup/readdir cannot see .sandbox at the project root; direct access to /home/hsaksela/ai/wrapper/.sandbox/... fails from the guest; required live-smoke or another required live test asserts .sandbox is not visible; offline composed-fs/runtime-manifest tests cover the exclusion behavior without hiding user project files.


## Notes

**2026-05-18T20:37:07Z**

Implemented workspace .sandbox hiding through composed-fs manifest filters: vm-frontend/src/runtime_manifest.rs now emits a shadow_root under the run dir and a m0001_workspace filter for suffix .sandbox with action hide-and-shadow. Added self-test payload assertion that .sandbox is not visible in the guest workspace, and validate.sh setup-tool probes assert test ! -e "/home/hsaksela/ai/wrapper/.sandbox". While validating, found guest-init still mirrored logs through PROJECT_PATH/.sandbox; fixed by disabling guest log mirroring by default and adding opt-in --mirror-guest-logs to write to project .vmlogs/docker-vm/run instead. Offline tests pass; required validation now stops at live appliance freshness because docker/guest-init.sh changed and appliance rebuild requires sudo in this harness.
