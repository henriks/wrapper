---
id: wra-1esa
status: closed
deps: [wra-09v9]
links: []
created: 2026-05-17T10:18:48Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, cli, deletion]
---
# Option 1: remove non-config CLI compatibility surface

Remove compatibility-only command line behavior. The review called out legacy argv routing in vm-frontend/src/main.rs, the agentvm-frontend binary alias and wrap compatibility path, removed-flag handling, and stringly launch-argv rewriting. Compatibility only matters for .sandbox/config.json.

## Design

After the CLI/config split, delete old command aliases and removed flag shims unless they represent current product behavior. Prefer typed clap derive or typed command structs over building Vec<String> launch arguments. Do not preserve command line behavior solely for compatibility.

## Acceptance Criteria

The supported CLI surface is explicit in clap help and tests, obsolete aliases and removed flags are gone, wrapper-to-launch conversion is typed rather than stringly, and .sandbox/config.json compatibility remains intact.


## Notes

**2026-05-17T18:44:42Z**

Continuation-2 iteration 16: started after closing wra-6n71. First slice will remove compatibility-only removed-flag shims from wrapper CLI parsing. Project guidance says compatibility only matters for .sandbox/config.json, so old command-line flags should not be retained solely to print custom migration messages. Keep config migration tests intact.

**2026-05-17T18:45:48Z**

Continuation-2 iteration 16 first slice: removed wrapper compatibility-only pre-scan for --docker, --docker-machine, and --pass-env. These flags are no longer special-cased to print custom migration messages; clap now rejects them as ordinary unknown arguments, same as other removed wrapper flags (--tool/--tool-arg/--command). This follows the project rule that only config-file compatibility matters. Config migration code/tests are unchanged. Validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline wrapper_rejects_removed_flags_as_unknown_arguments -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline wrapper_ -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T18:48:51Z**

Continuation-2 iteration 17: removed the explicit wrap compatibility subcommand dispatch. agentvm-frontend wrap now fails as unknown command, and agentvm wrap is treated as an invalid wrapper argument rather than a supported alias. Help text no longer advertises 'Compatibility: agentvm-frontend wrap'. This deliberately leaves .sandbox/config.json compatibility/migration untouched. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline wrap_subcommand_is_not_a_compatibility_entrypoint -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline wrapper_ -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T18:52:03Z**

Continuation-2 iteration 17 follow-up: also removed the stale documentation/help surfaces for the old wrap path. requirements.md now lists only agentvm-frontend launch/self-test as low-level frontend subcommands and explicitly says agentvm-frontend wrap is unsupported. Wrapper clap argv construction now always uses agentvm as the displayed command name instead of retaining an internal agentvm-frontend wrap display alias. Re-ran the focused wrapper tests and fmt successfully.

**2026-05-17T18:55:12Z**

Continuation-2 iteration 18: started reducing stringly wrapper-to-launch construction. Added a local WrapperLaunchFlag enum in vm-frontend/src/wrapper.rs for all launch flags emitted by wrapper parsing/config defaults/TLS bootstrap/payload script setup, plus small push/has/upsert helpers. The wrapper still serializes to Vec<String> at the run_launch boundary, but flag spelling is now centralized and typed inside the wrapper conversion path instead of scattered literal strings. This does not change .sandbox/config.json semantics. Validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline wrapper_args_translate_to_launch_args -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline wrapper_ -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T18:57:33Z**

Continuation-2 iteration 19 cleanup before required validation: removed the last wrapper parser/runner dependence on program names. run_wrapper now takes only wrapper args, parse_wrapper_args/parse_wrapper_args_with_terminal no longer accept an argv0/program parameter, and main.rs dispatch no longer passes the program name into wrapper parsing. This preserves the supported primary behavior (argv0=agentvm chooses wrapper mode at top-level dispatch) while keeping wrapper parsing itself free of legacy executable-name/alias compatibility. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline wrapper_ -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline wrap_subcommand_is_not_a_compatibility_entrypoint -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check.

**2026-05-17T18:59:42Z**

Continuation-2 iteration 19 validation: ./vm-frontend/validate.sh required passed after the CLI compatibility deletions and wrapper-to-launch cleanup. This covers formatting/offline tests, live-smoke, and live setup-tool bootstrap/persistence scenarios on the default Python appliance. Rust opt-in guest-service live validation remains outside this ticket and is still tracked by wra-662v/wra-y335.
