---
id: wra-k3di
status: closed
deps: [wra-9udq, wra-96uv, wra-mvxd]
links: []
created: 2026-05-14T18:43:26Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-tad5
tags: [validation, testing, docs, ci]
---
# Document and wire validation tiers into developer workflow

Document how to run each validation tier and wire practical commands into the developer workflow. This should avoid making normal development painfully slow while making exhaustive validation discoverable and repeatable.\n\nDocument commands for fast offline tests, ignored local integration tests, stress/model tests with seeds, and live KVM/QEMU tests. Include prerequisites for /dev/kvm, qemu-system-x86_64, appliance artifacts, Docker image rebuild expectations, network access expectations, and where artifacts are written.\n\nIf CI is available or planned, define which tiers belong in per-commit CI, pre-merge/manual CI, and nightly/full validation. The documentation should also state how to add new tests to the matrix and how tickets should record outcomes before close.

## Acceptance Criteria

README/operations or validation docs include exact commands, expected runtimes, prerequisites, artifact paths, and triage guidance. The dependency graph has no cycles. The epic can be closed only after child tickets document their outcomes and remaining gaps.


## Notes

**2026-05-14T20:05:10Z**

Added vm-frontend/validate.sh with fast, stress, all-local, and live tiers. Added vm-frontend/validation-workflow.md documenting exact commands, prerequisites, expected use, seeds, artifact paths, CI policy, and ticket outcome requirements. Linked it from README and validation-matrix. Verified vm-frontend/validate.sh fast and vm-frontend/validate.sh stress both pass locally; live remains host/KVM-only and was already reported passing under wra-9udq.
