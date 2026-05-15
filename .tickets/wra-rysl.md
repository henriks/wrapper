---
id: wra-rysl
status: closed
deps: [wra-reoq]
links: []
created: 2026-05-15T06:51:05Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-2cfd
tags: [filesystem, fuzzing, proptest]
---
# Expand composed-fs generators for hostile path and metadata edge cases

Broaden the composed-fs property generators beyond the current fixed pNNN.txt and eight-path nested model. Relevant code: composed-fs/src/lib.rs tests around FsStressOp, NestedFsOp, fs_stress_ops, nested_fs_ops, run_flat_file_ops_case, run_nested_fs_ops_case. Add generated cases for boundary-length names, varied valid byte content in file names where Unix permits it, long paths, deep-ish directory trees, dot/dotdot/slash rejection probes, hardlink and symlink graph cases, rename-over-file and rename-over-directory permutations, xattr/setattr/truncate/fsync/flush where supported, file mounts, overlapping/nested mounts, and readonly/rw boundary combinations. Preserve shrinkability and operation trace output.

## Acceptance Criteria

The composed-fs property suite covers richer path/name/metadata spaces while remaining deterministic and shrinkable. Failures print enough operation trace context to reproduce. Existing host-oracle comparisons are extended where practical, and unsupported platform-specific behavior is gated or asserted with clear errno expectations.


## Notes

**2026-05-15T07:02:46Z**

Expanded composed-fs nested operation generators from 8 to 16 path shapes, adding hidden, uppercase, space-containing, punctuation, and long-name paths. Added truncate and chmod operations to the host-oracle nested model with matching FUSE setattr helpers. Also made test temp directories unique per proptest case to avoid stale path collisions during shrinking. Verified with cargo test --manifest-path composed-fs/Cargo.toml --offline.

**2026-05-15T07:10:33Z**

During final stress validation, proptest_nested_operation_sequences_stress found that randomly chmodding directory paths to 0600 made host read_dir and composed-fs traversal semantics diverge. Adjusted the chmod generator to target file-shaped paths only; reran vm-frontend/validate.sh stress successfully.
