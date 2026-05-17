---
id: wra-r070
status: closed
deps: [wra-09v9]
links: []
created: 2026-05-17T10:18:54Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, errors]
---
# Option 1: replace stringly errors with typed domain errors

Replace internal Result<_, String> plumbing with typed errors before introducing broad async code. Current vm-frontend paths erase source errors and context in CLI/config/launch/self-test code, while lower modules already use thiserror in places.

## Design

Introduce focused error enums such as CliError, ConfigError, SupervisorError, PayloadError, VmnetError, and keep LaunchError where appropriate. Preserve source errors, paths, command context, child status, policy context, and timeout information. Format into user-facing strings only at the binary boundary.

## Acceptance Criteria

Internal command/config/launch/payload/vmnet APIs no longer convert rich errors to String mid-stack, tests assert structured failures where practical, and user-facing output remains clear.


## Notes

**2026-05-17T11:47:11Z**

Starting after wra-09v9 closure. Plan: keep slices mechanical and behavior-preserving; first inventory Result<_, String> boundaries in the newly extracted CLI/config/launch modules, then introduce focused typed errors starting at narrow module seams where tests already assert behavior. Preserve user-facing formatting at main/command boundary and avoid config semantic changes.

**2026-05-17T11:50:49Z**

Iteration 14 started typed-error work. Inventory found stringly Result boundaries concentrated in vm-frontend binary modules plus a few lower protocol helpers. First behavior-preserving slice converted vm-frontend/src/config.rs to a typed ConfigError/ConfigResult with structured variants for setup-tool parsing, share-shadow validation, schema/version validation, config read/parse/validate/write, setup-tool mise writes, and JSON/io sources. Existing callers still bridge to String through a temporary From<ConfigError> for String while broader CLI/launch/wrapper errors are migrated. Added/updated a config share-shadow test to assert structured ConfigError variants in addition to user-facing messages. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline config_share_shadow_validation -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline config_ -- --nocapture.

**2026-05-17T11:53:25Z**

Iteration 15 typed-error slice: converted vm-frontend/src/payload_cli.rs to PayloadCliError/PayloadCliResult for clap parse wrapping, numeric option validation, env parsing, missing script/positive bounds checks, guest sync diagnostic exit failures, and underlying PayloadClientError. Converted vm-frontend/src/vmnet_cli.rs to VmnetCliError/VmnetCliResult for clap parse wrapping, missing socket, and no-net policy validation. Kept temporary From<...> for String bridges so top-level String-returning callers continue to format at the binary boundary while they are migrated. Added/updated tests to assert structured PayloadCliError::EmptyEnvKey and VmnetCliError::MissingSocket while preserving message text. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline payload_client_config_reports_structured_parse_errors -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_gateway_runtime_config_requires_socket -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline flush_guest_filesystems -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline payload_client -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_gateway_runtime_config -- --nocapture.

**2026-05-17T11:56:43Z**

Iteration 16 reflection: progress is good—workspace/main split are closed, and typed-error work now has module-level errors for config, payload CLI, and vmnet CLI. The small seam-by-seam approach is keeping behavior stable and tests focused. Main drag is temporary error bridging: wrapper/launch/self-test still return String, so some typed errors still stringify when crossing those boundaries. Adjustment: introduce top-level command errors before converting the largest launch/self-test modules, then chip away at remaining bridges. Continued work this iteration: added a main.rs CliError/CliResult boundary so run_cli/run preserve PayloadCliError, VmnetCliError, PayloadClientError, prepare LaunchError, and vmnet runtime debug context until the binary boundary; wrapper still stringifies when invoking run from wrapper mode. Full vm-frontend offline validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline.

**2026-05-17T11:59:08Z**

Iteration 17 typed-error slice: converted vm-frontend/src/appliance.rs to ApplianceError/ApplianceResult for artifact manifest read/parse, repository-root inference, missing/invalid source hash metadata, changed source hashes, missing required inputs, and source file reads. Converted vm-frontend/src/tls_bootstrap.rs to TlsBootstrapError/TlsBootstrapResult for MITM CA directory creation, rcgen key/params/cert generation, certificate write, private-key write, and permission restriction. Existing callers still bridge these module errors to String while launch/wrapper seams are stringly. Updated appliance tests to assert structured ApplianceError variants for changed source hashes and missing source metadata. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline appliance_source_hashes -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline wrapper_mitm_ca -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.  confirms appliance.rs and tls_bootstrap.rs no longer expose Result<_, String>.

**2026-05-17T11:59:38Z**

Correction to prior iteration 17 note: the final verification was `rg -n "Result<[^>]+String|Result<.*, String>|-> Result<.*String" vm-frontend/src/appliance.rs vm-frontend/src/tls_bootstrap.rs || true`, which produced no matches for those two files.

**2026-05-17T12:01:16Z**

Iteration 18 typed-error slice: introduced WrapperError/WrapperResult in vm-frontend/src/wrapper.rs and changed run_wrapper to return typed errors. Wrapper dispatch now preserves config and TLS bootstrap errors through WrapperError and into main CliError, and no longer calls the top-level run() then stringifies CliError; it delegates directly to run_launch for both plain and TUI modes. apply_configured_launch_defaults/config_share_shadow_backing_path now return WrapperResult for typed config/path/share-shadow errors. Remaining known bridge: parse_wrapper_args_with_terminal still returns String for parser-focused tests and stringifies apply_configured_launch_defaults failures inside that parser seam; launch_cli/self_test remain stringly. Validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline wrapper_ -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline argv0_no_longer_selects_wrapper_or_tool_behavior -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline.

**2026-05-17T12:03:09Z**

Iteration 19 typed-error slice: introduced LaunchCliError/LaunchCliResult in vm-frontend/src/launch_cli.rs and changed run_launch to return typed launch CLI errors. Top-level CliError and WrapperError now carry LaunchCliError transparently, so wrapper/plain launch dispatch no longer stringifies launch results at the boundary. LaunchCliError currently preserves PayloadClientError, PayloadCliError, and ApplianceError plus a temporary Message(String) catch-all for still-stringly launch helpers; deeper launch helpers such as frontend_config_from_args/runtime_mounts/guest_payload_env still need further typed conversion. Validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check; cargo test --manifest-path vm-frontend/Cargo.toml --offline frontend_ -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline wrapper_ -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline.

**2026-05-17T12:05:48Z**

Iteration 20 typed-error slice: pushed LaunchCliError deeper through frontend_config_from_args, launch_payload_args, runtime_mounts, guest_payload_env, parse_guest_path_share, and parse_guest_path_share_shadow. Added structured LaunchCliError variants for frontend config loading, qemu/payload numeric parse errors, payload env format/empty key, empty payload script, share shadow parent/backing validation, share shadow backing mkdir, and ConfigError propagation. Remaining LaunchCliError::Message(String) catch-all is now concentrated around shared parse/path helpers, project lock/reset, local smoke upstream, readiness/no-net helpers, and final qemu status formatting. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline frontend_ -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline wrapper_ -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline self_test_payload -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check. Full vm-frontend offline validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline.

**2026-05-17T12:07:37Z**

Iteration 21 reflection: accomplishments are substantial: workspace and main split are closed after required validation, and typed errors now cover config, payload CLI, vmnet CLI, appliance freshness, TLS bootstrap, wrapper dispatch, top-level command dispatch, and most launch config/mount/env parsing. The seam-by-seam approach is working well: behavior-preserving changes have stayed small enough for focused tests, and repeated full vm-frontend offline validation remains green. The main remaining drag is diminishing returns in tiny shared helpers that still return String (, , AWS credential extraction, readiness probe formatting, self_test.rs internals). Adjustment: avoid overfitting every helper before live validation; finish one more small high-value launch slice, then run required validation and consider wra-r070 closable if it passes. Iteration 21 continued by converting project lock/reset, local HTTP smoke upstream setup, and no-net validation to typed LaunchCliError variants; vmnet_cli now stringifies only when adapting the shared no-net validator to VmnetCliError. Validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; focused tests for frontend_no_net, reset_project, wrapper_, vmnet_gateway_runtime_config; and full cargo test --manifest-path vm-frontend/Cargo.toml --offline.

**2026-05-17T12:07:43Z**

Correction to iteration 21 reflection note: the remaining tiny shared helpers referenced were `parse_clap_matches`, `absolute_cli_path`, AWS credential extraction, readiness probe formatting, and self_test.rs internals.

**2026-05-17T12:10:57Z**

Required live-capable validation for typed-domain-errors slice passed: ./vm-frontend/validate.sh required completed successfully, including docs/fmt/offline tests, fuzz target compilation, live-smoke, and live-setup-tools. No appliance rebuild was requested. Remaining stringly seams are intentionally narrow shared-helper/final-formatting adapters (parse_clap_matches, absolute_cli_path, AWS credential extraction, readiness probe text, and self_test internals) and do not block wra-r070 acceptance because major command/config/launch/payload/vmnet paths now preserve typed errors to the binary boundary.
