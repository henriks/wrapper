---
id: wra-yxtb
status: open
deps: [wra-reoq, wra-k8br, wra-rysl, wra-12nu, wra-735i, wra-o7ax]
links: []
created: 2026-05-15T06:51:46Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-2cfd
tags: [docs, validation, fuzzing]
---
# Document fuzzing workflow, corpora, and CI policy

Document the final fuzz/property/stress workflow once the implementation tickets land. Relevant docs: vm-frontend/validation-workflow.md, vm-frontend/validation-matrix.md, vm-frontend/README.md if needed. Include exact commands for fast, stress, coverage-guided fuzz smoke runs, long-running local fuzzing, corpus/crash minimization, seed handling, and which artifacts should be attached to tickets. Clarify what runs per-commit, pre-merge/manual, and nightly/host-only, and distinguish proptest regression failures from coverage-guided fuzz crashes.

## Acceptance Criteria

Docs describe how to run, reproduce, minimize, and triage failures from the expanded fuzzing strategy. The validation matrix maps the new tests and fuzz targets to filesystem/network risk areas. The CI/manual tier recommendation is explicit and does not require unavailable host capabilities for normal fast checks.

