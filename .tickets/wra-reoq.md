---
id: wra-reoq
status: open
deps: []
links: []
created: 2026-05-15T06:50:51Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-2cfd
tags: [validation, fuzzing, workflow]
---
# Wire complete fuzz and stress validation tier

Fix the current validation workflow so the documented stress tier actually runs every intended ignored stress/property test. Relevant files: vm-frontend/validate.sh, vm-frontend/validation-workflow.md, vm-frontend/validation-matrix.md. Existing ignored tests that are not currently included in vm-frontend/validate.sh stress include composed-fs::tests::proptest_nested_operation_sequences_stress, composed-fs::tests::proptest_lock_operation_sequences_stress, vmnet_gateway::tests::stress_seeded_generated_guest_frames, and vmnet_stream::tests::stress_many_chunked_frame_splits. Keep the tier offline and deterministic; avoid adding KVM, QEMU, Docker, network, or root requirements to this tier.

## Acceptance Criteria

vm-frontend/validate.sh stress invokes all intended ignored local stress/property tests or explicitly documents any excluded test and why. validation-workflow.md lists the exact commands and expected reproduction artifacts. validation-matrix.md reflects the updated coverage. Running tk dep cycle reports no dependency cycles after ticket wiring.

