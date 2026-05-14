---
id: wra-ay63
status: closed
deps: []
links: []
created: 2026-05-14T20:12:26Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-f16x
tags: [tests, fuzzing, rust]
---
# Add shared property/fuzz test harness plumbing

Add reusable test harness plumbing for randomized bug-finding tests in the Rust crates. This should support deterministic local runs in normal cargo test where practical, plus ignored heavier stress/fuzz entry points for longer runs. Current state: composed-fs already has proptest as a dev-dependency and ignored flat-file stress/property tests in composed-fs/src/lib.rs. vm-frontend currently has many example-driven unit tests but no proptest/cargo-fuzz style dependency. Relevant manifests: composed-fs/Cargo.toml and vm-frontend/Cargo.toml.

## Design

Decide the lowest-friction setup for this repo: likely proptest-based unit tests for deterministic invariants, with ignored stress variants using fixed seeds/case counts. If adding cargo-fuzz/libFuzzer targets, keep them optional and documented so normal cargo test stays fast. Prefer reusable generators/builders for Ethernet frames, IPv4/UDP/TCP packets, DNS payloads, guest paths, and composed-fs operation traces.

## Acceptance Criteria

- vm-frontend has the dev-dependencies needed for property-style tests if chosen.
- Shared helper functions or modules exist where they avoid duplicating packet/path generation logic.
- Heavy tests are marked ignored with commands in the ignore reason.
- Normal cargo test remains fast and deterministic.


## Notes

**2026-05-14T20:17:34Z**

Implemented the shared frontend test plumbing needed by the dependent network tickets: added proptest as a vm-frontend dev-dependency, added reusable packet constants/builders for UDP, DNS, TCP SYN, IPv6, unknown ethertypes/protocols, ARP replies, parse_tcp_frame, and a ScriptedStream/ReadStep helper for chunked/nonblocking QEMU frame IO tests. Verified with cargo test --manifest-path vm-frontend/Cargo.toml test_support::tests --offline.
