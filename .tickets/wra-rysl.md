---
id: wra-rysl
status: open
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

