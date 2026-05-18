---
id: wra-fpy2
status: closed
deps: []
links: [wra-t9s7, wra-b9b1]
created: 2026-05-18T10:36:28Z
type: task
priority: 3
assignee: Henrik Saksela
parent: wra-emj5
tags: [cleanup, docs]
---
# Prune stale removed-path references from active docs and comments

Some active docs/comments still refer to removed implementation paths such as guest-payload-server.py, future Rust guest service language, old Python frontend paths, or historical q35 fallback details. Historical spike documents can remain historical, but active runtime contracts and crate comments should reflect the current architecture.

## Design

Audit payload-protocol/src/lib.rs, docker/OPERATIONS.md, docker/runtime-contract.md, vm-frontend/vmnet-runtime-validation.md, docker/README.md, and other active docs. Remove or update stale references to deleted Python payload-service/frontend paths. Keep historical spike docs only if clearly marked as historical and not used as current contract.

## Acceptance Criteria

Active docs and crate comments describe the current Rust guest-service, supervisor, vmnet, and appliance paths; stale removed-path names are gone from current contract docs; documentation checks and required validation docs tier are recorded before close.


## Notes

**2026-05-18T12:02:24Z**

Iteration 13 cleanup: pruned stale removed-path references from active docs/comments. Updated payload-protocol crate docs to name the Rust guest service instead of docker/guest-payload-server.py/future Rust service; updated vmnet-runtime-validation to remove the Python frontend replacement phrasing and refer to Rust Docker bridge/payload protocol; updated OPERATIONS to say Rust guest Docker bridge; clarified the startup measurement's removed q35 fallback as a legacy per-share baseline. Audit command now finds no active guest-payload-server.py, guest-socket-bridge.py, agentvm-socket-bridge, socket-bridge.log, future Rust guest, Python frontend, old Python, or q35 fallback references outside historical wording. Validation: cargo fmt --manifest-path payload-protocol/Cargo.toml; cargo check --manifest-path payload-protocol/Cargo.toml; ./vm-frontend/validate.sh docs; full ./vm-frontend/validate.sh required passed earlier in this iteration after the vmnet code changes.
