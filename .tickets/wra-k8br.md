---
id: wra-k8br
status: closed
deps: [wra-reoq]
links: []
created: 2026-05-15T06:50:58Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-2cfd
tags: [fuzzing, rust, harness]
---
# Add coverage-guided fuzz harness infrastructure

Add a repo-local coverage-guided fuzzing setup for the Rust crates, such as cargo-fuzz/libFuzzer or a clearly documented equivalent if cargo-fuzz is unsuitable for the offline workflow. The goal is infrastructure first, not exhaustive target coverage. Relevant crates: composed-fs and vm-frontend. Candidate targets from the current code review: vm-frontend/src/vmnet_stream.rs QemuFrameIo read/write framing, vm-frontend/src/dns_proxy.rs DNS payload parsing and policy decisions, vm-frontend/src/vmnet_gateway.rs guest frame ingestion, composed-fs manifest/path validation and safe traversal helpers. Include corpus/crash artifact layout and commands that do not interfere with normal cargo test.

## Acceptance Criteria

A developer can run at least one coverage-guided fuzz target locally from documented commands. The fuzz workspace layout is checked in, corpora/crashes/artifacts policy is documented, and the normal fast test tier remains offline and unaffected. The first target should prove the harness can build and exercise code from either vm-frontend or composed-fs.


## Notes

**2026-05-15T07:00:39Z**

Added vm-frontend/fuzz cargo-fuzz-compatible harness infrastructure with target vmnet_stream_frame_io, corpus directory, artifact ignore rules, and README command notes. Verified with cargo check --manifest-path vm-frontend/fuzz/Cargo.toml after fetching libfuzzer-sys/arbitrary dependencies outside the sandbox; normal vm-frontend cargo test remains unaffected.
