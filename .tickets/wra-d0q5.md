---
id: wra-d0q5
status: closed
deps: []
links: [wra-6n71]
created: 2026-05-18T05:36:54Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-9m5h
tags: [cleanup, self-test, validation]
---
# Move remaining frontend self-test harness out of production binary

wra-6n71 removed one production self-test surface, but the frontend binary still carries self-test harness code and command plumbing. Production launch binaries should not contain live validation harnesses. Move remaining agentvm-frontend self-test behavior into validation-only tooling, scripts, or a dedicated test binary, then delete the production command path.

## Design

Use wra-6n71 as prior context, but do not reopen it. Audit vm-frontend self_test modules, command dispatch, validate.sh callers, and live scenario scripts. Keep validation behavior available through ./vm-frontend/validate.sh tiers while removing the production CLI surface and unused runtime glue.

## Acceptance Criteria

agentvm-frontend production command dispatch no longer exposes self-test harness behavior; validation scripts still run the same live/offline scenarios through test-only tooling; obsolete self-test production modules are deleted or cfg(test)-only; required validation is recorded before close.


## Notes

**2026-05-18T06:48:39Z**

Initial audit: production wrapper argv0 already rejects self-test, but agentvm-frontend still exposes a self-test subcommand in vm-frontend/src/main.rs and validate.sh invokes target/debug/agentvm-frontend self-test. Harness code lives in vm-frontend/src/self_test.rs and self_test_payload.rs; usage text and run_cli dispatch still include self-test. Direction for implementation: move this command into validation-only tooling or a dedicated test binary while keeping validate.sh scenarios working, then remove self-test from production agentvm-frontend dispatch/usage.

**2026-05-18T06:55:37Z**

Moved live self-test harness out of production command dispatch. self_test/self_test_payload modules are compiled only with the validation-self-test feature; production agentvm and agentvm-frontend argv0 reject self-test, and agentvm-frontend usage no longer lists it. Added dedicated validation-only bin agentvm-self-test (required-features=[validation-self-test]) and changed validate.sh live self-test scenarios to run it directly. Updated validation-workflow.md equivalent command and adjusted vm-frontend offline validation to run tests with validation-self-test so harness unit tests remain covered. Evidence: cargo run --features validation-self-test --bin agentvm-self-test -- --help prints dedicated usage; cargo run --bin agentvm-frontend -- self-test fails with unknown command; cargo build --bin agentvm-frontend succeeds without validation-self-test; ./vm-frontend/validate.sh required passed with live-smoke and live-setup-tools (full log /tmp/pi-bash-04b8bb9e71aa15b0.log).
