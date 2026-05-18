---
id: wra-w8ea
status: closed
deps: []
links: [wra-8xsb]
created: 2026-05-18T10:36:09Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-emj5
tags: [cleanup, composed-fs, fuzz, tests]
---
# Split composed-fs namespace and deduplicate test/fuzz helpers

wra-8xsb extracted host_ops, but composed-fs/src/lib.rs still carries namespace model, FileSystem implementation, helper functions, and thousands of lines of tests. composed-fs/src/test_support.rs and fuzz_harness.rs also duplicate helpers such as manifest construction, lookup, and in-memory readers/writers. Continue structural cleanup without changing behavior.

## Design

Split namespace/node/mount runtime types out of lib.rs where that deletes coupling. Move large test modules into focused files or shared test_support. Reuse the same helper constructors between unit tests and fuzz harness where practical. Remove allow(dead_code) from test_support by either using helpers directly or narrowing visibility. Keep the bounded blocking filesystem model intact.

## Acceptance Criteria

composed-fs lib.rs no longer owns namespace, FUSE ops, and all large tests in one file; duplicated test/fuzz helpers are merged; no production behavior changes except clearer structure; composed-fs tests, fuzz target compilation, and required validation are recorded before close.


## Notes

**2026-05-18T12:14:47Z**

Started cleanup. Initial audit: composed-fs/src/lib.rs is ~6.9k lines and owns namespace runtime plus FileSystem ops plus large unit/property tests. composed-fs/src/fuzz_harness.rs duplicates test_support helpers: TestDir, ctx, manifest_with_mounts, dir_mount, lookup, VecReader, VecWriter. First step is to make shared test_support available to the fuzzing feature and remove those duplicated fuzz helpers before moving larger namespace/test modules.

**2026-05-18T12:18:28Z**

Progress: shared composed-fs fuzz/unit scaffolding. Made composed-fs/src/test_support.rs available for cfg(any(test, feature=fuzzing)), removed module-wide allow(dead_code), narrowed test-only helpers, and changed fuzz_harness.rs to reuse test_support TestDir/ctx/manifest_with_mounts/dir_mount/lookup/VecReader/VecWriter instead of carrying duplicate definitions. Verification: cargo fmt --manifest-path composed-fs/Cargo.toml; cargo check --manifest-path composed-fs/Cargo.toml --features fuzzing; cargo test --manifest-path composed-fs/Cargo.toml --lib fuzz_harness --features fuzzing; cargo test --manifest-path composed-fs/Cargo.toml --lib. No appliance inputs touched.

**2026-05-18T12:21:05Z**

Progress: split composed-fs namespace runtime out of lib.rs. Added composed-fs/src/namespace.rs for NodeKind/Node/MountRuntime/HostIdentity/Namespace and moved namespace construction/lookup/cache/shadow helpers there. lib.rs now keeps ComposedFs/FileSystem ops and tests rather than owning namespace runtime too. Verification: cargo fmt --manifest-path composed-fs/Cargo.toml; cargo check --manifest-path composed-fs/Cargo.toml; cargo check --manifest-path composed-fs/Cargo.toml --features fuzzing; cargo test --manifest-path composed-fs/Cargo.toml --lib; cargo test --manifest-path composed-fs/Cargo.toml --lib fuzz_harness --features fuzzing. Current line split: lib.rs 6053, namespace.rs 869, fuzz_harness.rs 835, test_support.rs 195.

**2026-05-18T12:22:07Z**

Follow-up in same namespace split: removed the stale MountRuntime allow(dead_code); cargo check --manifest-path composed-fs/Cargo.toml still passes. Final current line split after this iteration is lib.rs 6053, namespace.rs 868, fuzz_harness.rs 835, test_support.rs 195.

**2026-05-18T12:25:04Z**

Progress: moved the large composed-fs unit/property test module out of lib.rs into composed-fs/src/tests.rs, leaving lib.rs as the production module surface plus small mod declarations. Tests now reuse shared test_support TestDir, ctx, lookup, VecReader, and VecWriter directly; local manifest/dir helpers are thin wrappers over test_support to preserve call-site readability without duplicate construction logic. Verification this iteration: cargo fmt --manifest-path composed-fs/Cargo.toml; cargo check --manifest-path composed-fs/Cargo.toml; cargo check --manifest-path composed-fs/Cargo.toml --features fuzzing; cargo test --manifest-path composed-fs/Cargo.toml --lib; cargo test --manifest-path composed-fs/Cargo.toml --lib fuzz_harness --features fuzzing. Also attempted ./vm-frontend/validate.sh composed-fs but that tier does not exist; use fast/fuzz-check/required for final validation. No appliance inputs touched.

**2026-05-18T12:28:26Z**

Completion validation: ./vm-frontend/validate.sh required passed with timeout 300s after composed-fs namespace/test split and test/fuzz helper deduplication. Evidence includes formatting, composed-fs/guest-service/payload-protocol/vm-frontend offline tests, offline guest service tests, fuzz target compilation, live-smoke, and live-setup-tools. Final structure: lib.rs keeps production ComposedFs/FileSystem surface; namespace.rs owns namespace/node/mount runtime; host_ops.rs owns confined host operation boundary; tests.rs owns large unit/property test module; test_support.rs is shared by tests and fuzz_harness under cfg(any(test, feature=fuzzing)). No appliance inputs touched; no sudo Docker appliance rebuild required.
