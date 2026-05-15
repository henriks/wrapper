---
id: wra-x2tl
status: closed
deps: []
links: []
created: 2026-05-15T08:40:15Z
type: task
priority: 1
assignee: Henrik Saksela
---
# Migrate frontend CLI parsing to clap

Replace the hand-rolled argument parsing in vm-frontend/src/main.rs with clap-backed parsers while preserving the UX contract: agentvm defaults to wrapper mode, agentvm-frontend keeps low-level subcommands, setup-tool recipes, post--- command override, compatibility --tool/--command, and existing tests. Add clap as direct vm-frontend dependency using the version already present in Cargo.lock. Keep parse_wrapper_args_with_terminal testable.

## Acceptance Criteria

cargo test --manifest-path vm-frontend/Cargo.toml --offline passes; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check passes; wrapper and low-level help/errors are generated or normalized through clap; tests cover post--- payload command and compatibility tool args.


## Notes

**2026-05-15T08:49:20Z**

Migrated CLI parsing to clap. vm-frontend now has clap as a direct dependency and uses clap command definitions for wrapper parsing plus launch/prepare, vmnet-gateway, payload-client, and self-test option parsing. The wrapper still splits post--- payload argv before clap so command override semantics remain exact. Removed the old value() helper and hand-rolled option loops. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check passed; agentvm --help smoke shows generated clap help plus explicit command override guidance.
