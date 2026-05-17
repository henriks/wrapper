---
id: wra-n0xc
status: open
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

