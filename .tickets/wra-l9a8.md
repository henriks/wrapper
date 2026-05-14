---
id: wra-l9a8
status: closed
deps: [wra-snm9, wra-l6ed]
links: []
created: 2026-04-01T21:17:14Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-oszw
tags: [vm, docs, requirements]
---
# Update docs and requirements for the VM-only sandbox model

Bring the written contract and operator docs in line with the VM-only rewrite. Current docs still describe Bubblewrap as a first-class part of the execution model even though the desired end state is to move the sandbox boundary into the VM itself.

Scope:
- Update `requirements.md` to describe the new primary architecture and supported flags.
- Update `docker/runtime-contract.md` and `docker/OPERATIONS.md` to match the implementation and the new lifecycle.
- Document prerequisites, verification steps, and expected runtime layout under `.sandbox/docker-vm/`.
- Remove historical/current confusion around Bubblewrap, host-side namespace semantics, and any no-longer-supported flags or modes.

Relevant docs:
- `requirements.md`
- `docker/runtime-contract.md`
- `docker/OPERATIONS.md`
- `docker/README.md`

This ticket should land after the implementation shape is clear enough that the docs are not speculative.

## Design

Prefer concise, current documentation over preserving historical detail. If a behavior or flag is no longer supported after the rewrite, remove it from the main docs instead of documenting both old and new models side by side.

## Acceptance Criteria

Docs and requirements match the implemented VM-only behavior.

Verification instructions are documented.

The main docs no longer describe Bubblewrap as part of the supported agent execution model.

## Notes

**2026-04-01T21:25:42Z**

Contract decisions from wra-fj7n are now written in docker/runtime-contract.md and reflected at the top of requirements.md. The docs cleanup ticket should remove the remaining Bubblewrap-era detail sections, fully rewrite the CLI/flags sections around the VM-only model, and make .sandbox/home/ plus the removed flags the documented steady state rather than an interim override.

**2026-04-01T21:27:09Z**

Docs follow-up from the contract correction: the steady-state docs should present --ro and --rw as supported VM-only guest-share features, and only --pass-env as removed. Current contract/requirements were updated accordingly.

**2026-04-02T07:06:26Z**

Implementation follow-up from wra-mkh8: docs will need to explain the new guest-share model explicitly. In the VM-only path, host-backed inputs are no longer described as Bubblewrap binds; they are provided through supplemental virtio-fs shares, including arbitrary --ro/--rw paths at the same guest path.

**2026-05-14T18:14:08Z**

Updated the main docs for the implemented VM-only Rust model. Rewrote requirements.md around agentvm-frontend/codex-wrap/copilot-wrap Rust invocation, supported VM-era flags, removed flags, project-local state, guest shares, payload environment, Rust vmnet networking, and verification. Rewrote docker/OPERATIONS.md to describe embedded composed-fs, QEMU stream vmnet, Rust-owned lifecycle, current runtime layout, reset, self-test, and troubleshooting. Updated docker/runtime-contract.md to remove stale virtiofsd/QEMU user networking/docker-data.meta assumptions and align startup/readiness/shutdown with the Rust frontend. Updated docker/README.md to describe the current appliance contents and guest init services. Updated vm-frontend/README.md with the one-command KVM self-test and non-KVM cargo test coverage. Verification: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed after the docs/self-test updates; grep of the main docs shows no remaining positive references to the old Bubblewrap or QEMU user-networking model.
