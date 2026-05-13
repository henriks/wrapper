---
id: wra-octf
status: open
deps: []
links: [wra-oszw]
created: 2026-05-13T10:19:15Z
type: epic
priority: 1
assignee: Henrik Saksela
tags: [rust, qemu, virtiofs, network, frontend]
---
# Build Rust frontend for VM, filesystem, and network orchestration

Pivot from the Python sandbox-wrap frontend toward a Rust frontend that owns VM orchestration. The Rust frontend should launch and supervise QEMU and companion tools, embed or directly reuse the composed filesystem implementation, and provide the place where upcoming network orchestration features will live. This epic should absorb the lessons from the completed microvm + composed virtio-fs work without preserving redundant legacy paths.

## Design

Initial direction: treat Rust as the process supervisor/frontend for QEMU and any helper tools. Reuse the existing composed-fs Rust code rather than shelling out to redundant Python-managed plumbing where practical. Keep QEMU command construction, runtime state layout, process lifecycle, logging, readiness checks, and network setup explicit and testable. Network requirements are intentionally not fully specified yet; create follow-up tickets once those details are described. Preserve the validated user-facing behavior from the current wrapper: microvm default, composed filesystem export, natural guest paths, Docker/payload readiness, --ro/--rw semantics, --no-net, and published localhost ports unless superseded by the new network design.

## Acceptance Criteria

A Rust frontend design is documented; implementation tickets cover process supervision, QEMU command construction, composed filesystem integration, runtime state/logging, readiness/control paths, and the described network features; dependencies reflect implementation order; tickets explicitly document outcomes so investigation work is not repeated. The plan should avoid duplicate legacy implementations and should identify what Python sandbox-wrap code will be retired or temporarily bridged.

