---
id: wra-gx4r
status: closed
deps: [wra-f76x]
links: []
created: 2026-05-14T18:41:10Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-tad5
tags: [validation, testing, harness]
---
# Build shared deterministic validation harnesses

Add shared test utilities so network and filesystem suites do not each invent duplicate scaffolding. The harness should support deterministic temp project roots, artifact manifests, runtime dirs, fake time where useful, fake upstream network peers, fake QEMU frame streams, generated CA fixtures, and guest-share fixture trees.\n\nRelevant code: vm-frontend tests currently live mostly inline in vm-frontend/src/*.rs; composed-fs has its own tests. Evaluate whether to add crate-local test support modules, integration tests, or reusable helpers with minimal production visibility.\n\nThe goal is to make later exhaustive tests concise and consistent, not to add another runtime code path.

## Acceptance Criteria

Reusable harness helpers exist for frontend config/runtime fixtures, network frame/proxy fixtures, CA fixtures, and filesystem fixture trees. Existing tests can use the helpers without changing production behavior. The ticket notes where helpers live and what later tickets should reuse.


## Notes

**2026-05-14T18:52:05Z**

Implemented reusable test-only harness modules. vm-frontend/src/test_support.rs provides TestTempDir, FrontendFixture, RuntimePaths fixture access, QEMU frame byte helpers, memory QEMU frame IO, deterministic smoltcp time, generated MITM CA fixtures, and a one-shot loopback TCP upstream. composed-fs/src/test_support.rs provides TestDir, GuestShareFixture, manifest/mount builders, lookup helpers, and zero-copy VecReader/VecWriter helpers. Existing launch/composed-fs tests now exercise the shared fixtures. Verification: cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo test --manifest-path composed-fs/Cargo.toml --offline.
