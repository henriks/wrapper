---
id: wra-a9nb
status: closed
deps: []
links: []
created: 2026-05-15T08:55:26Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-sne0
---
# Replace custom test temp dirs with tempfile

Use tempfile::TempDir for test and fuzz fixture directories instead of timestamp/PID path construction plus manual Drop cleanup. Relevant code: vm-frontend/src/test_support.rs TestTempDir, composed-fs/src/test_support.rs TestDir, composed-fs/src/fuzz_harness.rs TestDir, unique_temp_dir helpers in vm-frontend/src/main.rs, vm-frontend/src/launch.rs, vm-frontend/src/runtime_manifest.rs, vm-frontend/src/vmnet_runtime.rs, and composed-fs/src/lib.rs tests. This is a low-risk first cleanup that reduces duplicated fixture code and avoids temp path collisions.

## Acceptance Criteria

All custom timestamp/PID temp dir helpers in tests/fuzz support are replaced or justified. Tests do not assert exact temp path names. cargo test --manifest-path vm-frontend/Cargo.toml --offline and cargo test --manifest-path composed-fs/Cargo.toml --offline pass.


## Notes

**2026-05-15T09:18:13Z**

Implemented with tempfile across shared frontend/composed-fs test support, composed-fs fuzz/test fixtures, vm-frontend launch/runtime/main test roots, vmnet runtime log temp files, and pcap test temp files. Production timestamp use in PcapWriter remains for packet timestamps, not temp paths. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo test --manifest-path composed-fs/Cargo.toml --offline; fmt checks.
