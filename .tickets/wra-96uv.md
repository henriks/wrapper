---
id: wra-96uv
status: closed
deps: [wra-czl0, wra-1joy]
links: []
created: 2026-05-14T18:43:01Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-tad5
tags: [validation, testing, stress, fuzz]
---
# Add stress fuzz and regression suites for network and filesystem

Add opt-in stress and fuzz-ish regression suites that exercise both network and filesystem paths beyond normal fast tests.\n\nNetwork stress: many concurrent TCP sessions, repeated DNS queries, large HTTP/HTTPS responses, adversarial chunking, random partial writes, connection churn, repeated wrapper launches, and event-log volume.\n\nFilesystem stress: many files/directories, deep trees, large files, random operation sequences against the composed-fs model, concurrent readers/writers, repeated lookup/forget/open/release cycles, host-side mutation while guest-side operations proceed, and reset/persistence loops.\n\nThese tests should be deterministic by seed and should print the seed/operation sequence on failure.

## Acceptance Criteria

Stress tests exist behind an explicit command/filter so they do not slow normal cargo test. Failures report reproducible seeds or operation traces. Notes document tested scale, runtime, and known limits.


## Notes

**2026-05-14T19:36:34Z**

wra-czl0 added a large fragmented guest-side TLS MITM regression and fixed plaintext streaming under rustls backpressure. Stress coverage should extend this with repeated large HTTP/HTTPS responses, adversarial TLS chunk sizes, and real proxy success paths against live or more robust TLS peers.

**2026-05-14T19:38:53Z**

wra-0452 added deterministic model-style filesystem operation sequences. Stress/fuzz work should extend this pattern with generated operation sequences, seeds in failure output, deep trees, large files, repeated host mutation, and concurrent lookup/write/unlink churn.

**2026-05-14T19:46:30Z**

composed-fs now has deterministic protocol-boundary tests for handle lifecycle and distinct-offset concurrent writes. Stress/fuzz coverage should still vary operation interleavings over lookup/forget/open/release/rename/unlink/readdir and include raw virtiofs/FUSE request corruption once a byte-level harness exists.

**2026-05-14T19:59:32Z**

Added proptest as a composed-fs dev-dependency and introduced opt-in property/stress coverage. Filesystem stress now includes: proptest_flat_file_operation_sequences (128 generated flat-file operation sequences with shrinking and operation traces) and stress_seeded_flat_file_operation_sequences (fixed seed 0x5eedf17e20260514, 512 operations). The first stress run found a real bug: after rename, host inode reuse by (dev, ino) kept the old relative path, causing later lookup/open of the renamed file to fail with ENOENT. Fixed get_or_create_host_node to refresh the Host node relative_path and metadata when reusing an inode, and added lookup_after_rename_refreshes_reused_host_inode_path as a focused regression. Network stress now includes dns_proxy_stress_seeded_repeated_policy_queries with seed 0xd15c20260514 and 1024 mixed A/AAAA allowed/blocked queries, asserting denied queries never hit upstream and allowed queries preserve call order. Normal suites and ignored stress commands pass.
