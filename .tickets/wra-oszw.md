---
id: wra-oszw
status: open
deps: []
links: [wra-octf]
created: 2026-04-01T21:16:25Z
type: epic
priority: 1
assignee: Henrik Saksela
tags: [vm, sandbox, qemu, cleanup]
---
# Rewrite sandbox-wrap around a VM-only sandbox model

Replace the current hybrid host-Bubblewrap-plus-guest-Docker design with a single VM-only sandbox model. The agent process, its filesystem view, and Docker should all live inside the project-scoped VM. The host wrapper should become a thin lifecycle/orchestration layer rather than a second sandbox runtime.

Current context:
- `sandbox-wrap` already contains a substantial VM supervisor in `DockerVmManager` and guest appliance plumbing.
- The remaining complexity comes from preserving the old Bubblewrap execution model alongside the VM path.
- The current script is carrying two isolation models at once: `bwrap` for the agent and QEMU for Docker.
- Live debugging has shown the VM path is viable, while the hybrid model increases code volume and failure surface.

Goals for this epic:
- Drop Bubblewrap entirely for the primary execution path.
- Run Codex/Copilot and Docker inside the same guest.
- Keep the project-specific lifecycle and data disk under `.sandbox/docker-vm/`.
- Bias implementation toward deleting code and reducing moving parts.
- Add a repeatable end-to-end verification path that can validate the real VM flow on a host with KVM.

Key code areas:
- `sandbox-wrap`: current CLI, `DockerVmManager`, Bubblewrap execution path, environment/mount handling.
- `docker/build-appliance.sh` and `docker/guest-init.sh`: appliance build and guest boot behavior.
- `docker/runtime-contract.md`, `docker/OPERATIONS.md`, `requirements.md`: current design/docs that still describe the hybrid model.

This epic is done when the wrapper uses one isolation boundary only, the code structure is materially simpler than today, and there is a host-runnable self-test for the VM path.

## Design

Design constraints:
- No backwards-compatibility layer is required for the old Bubblewrap model.
- The VM remains project-scoped and tied to sandbox process lifetime.
- The rewrite should preserve necessary functionality only: project workspace sharing, auth/config access needed by the selected tool, Docker access, optional port publishing, and reset/cleanup semantics.
- Verification should be designed in, not left as ad hoc manual log inspection.
- The new structure should make most logic unit-testable without KVM, while also providing one real KVM smoke path for hosts that have it.

## Acceptance Criteria

Child tickets cover runtime contract, guest execution/control path, guest state/auth mapping, host launcher simplification, verification, and docs.

Dependencies reflect implementation order.

The final implementation uses the VM as the only sandbox boundary for the agent path.

## Notes

**2026-05-13T10:19:33Z**

Pivot note: new epic wra-octf tracks the Rust frontend direction. Future VM-only wrapper work should be evaluated against that pivot: avoid investing in Python sandbox-wrap cleanup that will be superseded by the Rust frontend, except where needed as a bridge or to preserve currently validated behavior during migration.
