---
id: wra-6n71
status: closed
deps: [wra-09v9]
links: [wra-d0q5]
created: 2026-05-17T10:20:02Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, self-test, cli, deletion]
---
# Option 1: move live self-tests out of production CLI

Move sprawling live self-test and shell/Python script generation out of the production CLI surface. The review identified run_self_test and validation harness code in vm-frontend/src/main.rs as bloat that obscures launch and runtime behavior.

## Design

Create an integration harness crate, xtask-style binary, or dedicated test support module for live scenarios. Production agentvm should launch and manage VMs, not embed large validation scripts. Keep validate.sh behavior intact or update it explicitly. Preserve live-smoke and setup-tool scenarios required by project validation.

## Acceptance Criteria

Production CLI no longer owns self-test script generation, validation entrypoints still work, live scenario code has focused ownership, and docs/scripts are updated for the new location.


## Notes

**2026-05-17T18:23:59Z**

Continuation-2 iteration 12: started while required validation for wra-662v/wra-gq8e/wra-73tn remains blocked on stale appliance artifacts. Chosen slice is intentionally low-risk/offline-testable: reduce production CLI ownership of live self-test harness code by moving self-test-specific unit coverage out of src/main.rs and into src/self_test.rs before considering any command-surface changes. validate.sh behavior must remain intact.

**2026-05-17T18:29:49Z**

Continuation-2 iteration 12 first cleanup slice: moved self-test-specific unit coverage out of the top-level main.rs test module and colocated it with vm-frontend/src/self_test.rs. Also moved default artifact-manifest path parsing ownership out of self_test.rs into launch_cli.rs because it is shared launch/self-test CLI infrastructure rather than live self-test script-generation logic. validate.sh behavior is unchanged. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline self_test -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline parses_frontend_prepare_defaults_and_policy -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check. Required validation also passed after appliance rebuild.

**2026-05-17T18:37:31Z**

Continuation-2 iteration 14: extracted vm-frontend/src/self_test_payload.rs from self_test.rs for live self-test payload script generation and hostile payload steps. self_test.rs now owns self-test CLI/orchestration and SQLite host-side helpers, while the shell/Python/Node guest script assembly has focused module ownership. This is still inside the production binary, so wra-6n71 remains open; next slices should move the validation command surface/harness out of production CLI or create a dedicated harness binary/module boundary. Validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline self_test_payload -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline self_test::tests -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check; and ./vm-frontend/validate.sh required after the user rebuilt appliance artifacts.

**2026-05-17T18:43:22Z**

Continuation-2 iteration 15: moved the live validation entrypoint off the production agentvm argv0. `agentvm self-test` now returns `unknown command: self-test`; live validation uses `cargo run --manifest-path vm-frontend/Cargo.toml --offline --bin agentvm-frontend -- self-test ...` instead. Updated vm-frontend/validate.sh plus README/validation-workflow examples accordingly. This preserves live scenario behavior while keeping the user-facing production wrapper CLI from owning/exposing the self-test command. Required validation passed after the change: ./vm-frontend/validate.sh required. Focused coverage also passed: agentvm_argv0_does_not_expose_self_test_command, self_test::tests, docs drift, and rustfmt check.

**2026-05-18T05:40:01Z**

Follow-up cleanup ticket wra-d0q5 was created under wra-9m5h for remaining frontend self-test harness code still exposed through production binary plumbing. Keep this ticket closed; wra-d0q5 owns the remaining production-surface deletion.
