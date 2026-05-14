---
id: wra-jwaz
status: open
deps: [wra-s8nn]
links: []
created: 2026-05-14T18:42:32Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-tad5
tags: [validation, testing, frontend, filesystem]
---
# Add runtime sharing and wrapper contract tests

Add exhaustive tests for the frontend/runtime contract around wrapper mode, payload launch, guest shares, CA bootstrap, and reset/persistence semantics.\n\nCover wrapper defaults, default absolute project/run-dir behavior, --tool codex/copilot, tool args, removed legacy flags, --no-net, --reset, public egress profile, generated per-project MITM CA, cert-only guest mount, private-key host-only guarantee, payload env including NODE_EXTRA_CA_CERTS/NPM_CONFIG_CAFILE, HOME/XDG paths, .docker, Codex/Copilot state dirs, optional gh, optional AWS env, explicit --ro/--rw, and Docker host env.\n\nThis ticket should include tests that prove the guest filesystem contract and wrapper contract compose correctly, without needing live QEMU for every case.

## Acceptance Criteria

Fast tests cover wrapper/runtime sharing behavior and ensure no redundant legacy code path is required. Notes document what is covered offline versus what is deferred to live VM tests.


## Notes

**2026-05-14T18:59:53Z**

Filesystem manifest validation now asserts guest config FS exposes the MITM CA certificate but not the private key, and runtime_manifest tests cover Copilot state dirs, Docker state, gh opt-in behavior, optional missing mounts, and explicit ro/rw path invariants. Wrapper contract tests should build on these invariants rather than rechecking raw manifest validation.
