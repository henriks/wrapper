---
id: wra-dmmq
status: closed
deps: []
links: []
created: 2026-05-18T10:35:30Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-emj5
tags: [cleanup, cli, tokio, vmnet]
---
# Collapse remaining sync CLI and vmnet runtime wrapper

agentvm main already starts a Tokio runtime, but vmnet-gateway and test dispatch still pass through sync CLI helpers. vm-frontend/src/main.rs retains run_cli with allow(dead_code), run() still owns sync command dispatch, and vm-frontend/src/vmnet_runtime.rs exposes serve_vmnet_gateway() that creates a nested Tokio runtime before calling serve_vmnet_gateway_async. Delete this remaining sync runtime island.

## Design

Convert command dispatch tests to run_cli_async or an async test helper. Route vmnet-gateway through an async command path from the existing top-level runtime. Delete serve_vmnet_gateway() if no external production caller needs it; otherwise make it test-only with explicit rationale. Consider splitting thin bin mains for agentvm, agentvm-frontend, and agentvm-self-test if that deletes argv0/string dispatch rather than adding more wrapper code.

## Acceptance Criteria

No production command path creates a nested Tokio runtime for vmnet; run_cli dead-code helper is removed or cfg(test)-only without allow(dead_code); cargo check no longer warns about the same main.rs being compiled as multiple bins if split bin mains are implemented; command dispatch tests still cover agentvm and agentvm-frontend behavior; required validation is recorded before close.


## Notes

**2026-05-18T10:44:06Z**

Iteration 1 progress: removed production sync CLI runtime island in vm-frontend/src/main.rs. vmnet-gateway now dispatches through run_async and calls serve_vmnet_gateway_async from the existing top-level Tokio runtime; deleted run_cli dead-code helper and the sync serve_vmnet_gateway wrapper in vm-frontend/src/vmnet_runtime.rs. Converted command dispatch tests to call run_cli_async. Verification so far: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --bin agentvm-frontend does_not_expose_self_test_command; cargo test --manifest-path vm-frontend/Cargo.toml --bin agentvm-frontend subcommand_is_not; cargo test --manifest-path vm-frontend/Cargo.toml --bin agentvm-frontend argv0_no_longer; cargo check --manifest-path vm-frontend/Cargo.toml --bins. Required validation still pending before close. No sudo appliance rebuild required for these changes.

**2026-05-18T10:48:05Z**

Iteration 2 cross-check: cargo check --manifest-path vm-frontend/Cargo.toml --lib --bin agentvm-frontend still succeeds after launch lifecycle refactor. Existing Cargo warning about src/main.rs being shared by agentvm/agentvm-frontend/agentvm-self-test remains; no bin-main split has been done yet.

**2026-05-18T10:52:55Z**

Iteration 3 completion: split the shared multi-bin main out of src/main.rs. The CLI implementation now lives in vm-frontend/src/cli_main.rs with thin bin entry files under vm-frontend/src/bin/{agentvm.rs,agentvm_frontend.rs,agentvm_self_test.rs}; Cargo.toml points each bin at a distinct file, eliminating the previous same-main.rs multi-bin cargo warning. Fixed validation-self-test module path after the split. Verification: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo check --manifest-path vm-frontend/Cargo.toml --bins (no same-main.rs warning); cargo test --manifest-path vm-frontend/Cargo.toml --bin agentvm-frontend does_not_expose_self_test_command; cargo test --manifest-path vm-frontend/Cargo.toml --bin agentvm-frontend subcommand_is_not; cargo test --manifest-path vm-frontend/Cargo.toml --bin agentvm-frontend argv0_no_longer; cargo check --manifest-path vm-frontend/Cargo.toml --bins --features validation-self-test; ./vm-frontend/validate.sh required passed after fixing the module path. No sudo appliance rebuild required.
