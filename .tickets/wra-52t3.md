---
id: wra-52t3
status: closed
deps: [wra-lxhx, wra-nrz2]
links: []
created: 2026-05-13T10:22:04Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, qemu, frontend, lifecycle]
---
# Scaffold Rust VM frontend supervisor

Create the Rust frontend that will become the process supervisor for QEMU, the composed filesystem backend, the userspace network gateway, and any remaining helper tools. This replaces the Python sandbox-wrap orchestration over time while preserving validated behavior: microvm default, composed filesystem, project-local runtime state, Docker/payload readiness, reset/cleanup, and logs.

## Design

Start with a narrow executable or crate that can build QEMU command lines from structured config, manage .sandbox/docker-vm/ runtime paths, launch/supervise child processes or in-process async tasks, write state.json-compatible status, and expose enough logging for failures. Use the existing Python wrapper as behavioral reference, not as a permanent dependency. Keep command construction unit-testable without KVM.

## Acceptance Criteria

A Rust frontend skeleton exists with runtime path/config structures, QEMU command construction for the validated microvm composed mode, process/log/state abstractions, and tests for command shape. It does not yet need full network gateway behavior, but it must leave explicit extension points for composed fs and vmnet tasks.


## Notes

**2026-05-13T10:36:04Z**

wra-nrz2 exposed composed-fs as a library. Supervisor should depend on agentvm_composed_fs and call serve_vhost_user_fs(ServeConfig) in a dedicated blocking task/thread, rather than spawning a separate long-term filesystem helper. The existing binary can remain only as a migration/diagnostic bridge.

**2026-05-13T10:38:58Z**

Supervisor scaffold added under vm-frontend/. It defines RuntimePaths, ToolPaths, VmArtifacts, GuestNetwork, VmShape, FrontendConfig, StateSnapshot, SupervisorPlan, ManagedTask, and ProcessSpec. build_microvm_qemu_command() emits validated microvm composed-fs command shape using stream vmnet and no QEMU usernet/hostfwd. The scaffold uses agentvm_composed_fs::ServeConfig for composed/config fs embedding and documents that host-to-guest access must be frontend-owned over vmnet. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed with 4 tests; composed-fs tests also passed after ServeConfig gained PartialEq/Eq.
