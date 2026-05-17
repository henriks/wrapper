---
id: wra-09v9
status: closed
deps: [wra-9glk]
links: []
created: 2026-05-17T10:18:43Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, cli]
---
# Option 1: split vm-frontend main into responsibility modules

Split vm-frontend/src/main.rs into focused modules before deep async work. Current main.rs mixes CLI dispatch, wrapper UX, .sandbox/config.json structs and migration, launch argument construction, self-test orchestration, TLS CA bootstrap, runtime mounts, local smoke servers, credential extraction, and payload execution.

## Design

Suggested modules: cli.rs for clap definitions and command enum, config.rs for .sandbox/config.json parsing/writing/validation/migration, wrapper.rs for wrapper command assembly, mounts.rs or env.rs for runtime mounts and auth/share resolution, self_test.rs for live checks, tls_bootstrap.rs for MITM CA setup, and paths/model modules as needed. Preserve config-file compatibility only. Update vm-frontend/config-json.md and tests in the same change if config semantics move or change.

## Acceptance Criteria

main.rs becomes a thin binary entry and dispatch layer, config parsing remains documented and tested, behavior is pinned with focused tests, and unrelated runtime code is not refactored in the same ticket.


## Notes

**2026-05-17T11:23:26Z**

Starting after wra-9glk closure. Initial inspection: vm-frontend/src/main.rs is 5,784 lines and currently contains CLI dispatch, wrapper argument parsing, config schema/migration, setup-tool config writing, launch argument construction, payload-client/vmnet/self-test clap parsers, runtime mounts/env, appliance freshness, local smoke helpers, and a large test module. Existing lib.rs already exports runtime/network/launch modules and keeps tui as a binary-local module. First implementation slice should be mechanical: extract cohesive code into binary modules without changing behavior or config semantics, then run focused tests after each move. Likely initial extraction order: config/setup-tool schema and JSON helpers; wrapper CLI/default launch assembly; self-test helpers. Avoid moving unrelated runtime behavior in the same diff.

**2026-05-17T11:25:24Z**

Iteration 5 extraction slice: created vm-frontend/src/appliance.rs for appliance artifact freshness responsibilities (manifest structs, required source input list, source-hash validation, sha256 helper). main.rs now imports ensure_appliance_sources_fresh and only exposes test-only imports for existing appliance freshness tests; no config semantics changed. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check; cargo test --manifest-path vm-frontend/Cargo.toml --offline appliance_source_hashes -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline frontend_config_from_args -- --nocapture.

**2026-05-17T11:27:39Z**

Iteration 6 reflection and extraction: the current approach is working when extracting cohesive, behavior-preserving binary responsibilities in small slices with focused validation. Added vm-frontend/src/payload_cli.rs for payload-client CLI parsing, payload exit-code mapping, and the sync diagnostic filesystem flush helper. main.rs still owns top-level dispatch but now delegates payload CLI support. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check; cargo test --manifest-path vm-frontend/Cargo.toml --offline payload_client -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline guest_sync -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline flush_guest_filesystems -- --nocapture.

**2026-05-17T11:29:53Z**

Iteration 7 extraction: added vm-frontend/src/vmnet_cli.rs for vmnet-gateway CLI parsing and VmnetRuntimeConfig construction. main.rs now delegates vmnet-gateway argument parsing while retaining top-level dispatch. This is behavior-preserving; no config semantics changed. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_gateway_runtime_config -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline frontend_no_net -- --nocapture.

**2026-05-17T11:33:35Z**

Iteration 8 larger extraction: added vm-frontend/src/config.rs for .sandbox/config.json/setup-tool schema, validation, migration, config file paths, config read/write, and setup-tool mise config writing. main.rs now delegates config parsing/writing and imports config data types. This was a mechanical move only; no config field semantics, defaults, migration behavior, or serialization changed, so config-json.md did not need semantic updates. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check; cargo test --manifest-path vm-frontend/Cargo.toml --offline setup_tool -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline config_share_shadow -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline legacy_config -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline config_schema -- --nocapture.

**2026-05-17T11:36:19Z**

Iteration 9 extraction: added vm-frontend/src/tls_bootstrap.rs for wrapper/project MITM CA generation and private-key writing. main.rs now delegates ensure_wrapper_mitm_ca and no longer owns rcgen/key-permission details. This is behavior-preserving; paths, certificate naming, key permissions, and payload CA env behavior are unchanged. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline wrapper_mitm_ca -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline payload_env_sets_guest_ca_bundle -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T11:38:29Z**

Iteration 10 extraction: added vm-frontend/src/self_test.rs for self-test config parsing, live self-test orchestration, self-test payload script construction, published-container check helper, and SQLite concurrency smoke helpers. main.rs delegates run_self_test and imports test-only helpers for existing binary tests. This is still a production CLI command (wra-6n71 owns moving live self-tests out of production CLI), but its implementation is no longer embedded in main.rs. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline self_test_payload -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline parses_self_test -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline reset_sqlite_concurrency_db -- --nocapture. Broader offline validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline.

**2026-05-17T11:40:32Z**

Iteration 11 reflection and extraction: reflection checkpoint confirmed the small mechanical extraction approach is working, but main.rs still needed a larger user-facing seam moved before considering acceptance. Added vm-frontend/src/wrapper.rs for wrapper command parsing/assembly, wrapper UI mode selection, setup-tool launch argument assembly, configured launch defaults, command override handling, project mise wrapping, and wrapper run dispatch. main.rs now imports run_wrapper/WrapperUiMode and keeps lower-level frontend launch/runtime helpers. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline wrapper_ -- --nocapture. Broader offline validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check; cargo test --manifest-path vm-frontend/Cargo.toml --offline.

**2026-05-17T11:43:37Z**

Iteration 12 extraction: added vm-frontend/src/launch_cli.rs for frontend launch/prepare CLI parsing, run_launch, project lock/reset helpers, runtime mount and guest env assembly, policy construction, local HTTP smoke upstream, payload readiness wait, and no-net validation. main.rs now primarily owns top-level dispatch, shared tiny parse/quoting/auth helpers, print_usage, and tests. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline frontend_ -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline wrapper_ -- --nocapture. Broader offline validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check; cargo test --manifest-path vm-frontend/Cargo.toml --offline.

**2026-05-17T11:46:36Z**

Required live-capable validation passed after the final main.rs extraction: ./vm-frontend/validate.sh required completed successfully, including fmt/docs/offline tests, fuzz compilation, live-smoke, and live-setup-tools. No appliance rebuild was requested. Acceptance criteria met: production main.rs is now a thin entry/dispatch layer while config parsing remains in config.rs with existing tests and unchanged documented semantics.
