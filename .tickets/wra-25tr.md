---
id: wra-25tr
status: closed
deps: []
links: []
created: 2026-05-15T07:48:21Z
type: feature
priority: 1
assignee: Henrik Saksela
---
# Persist sandbox project config and command defaults

The TUI wrapper currently reprompts for Codex/project sandbox configuration each time it starts. Persist per-project sandbox configuration in a config file, including the default command to run, and add a CLI switch to override the configured command so users can launch alternatives such as an interactive shell session.

## Acceptance Criteria

Configured projects are not prompted again on subsequent starts. The persisted sandbox config records the default command. A command-line switch can override the configured/default command for a single run. Tests or validation cover config persistence and command override behavior.


## Notes

**2026-05-15T07:53:06Z**

Implementation: added project-local .sandbox/config.json with schema_version, codex_enabled, and default_command. Wrapper now reads this config to avoid reprompting configured Codex projects, writes it when the TUI startup dialog accepts Codex, and supports --command CMD as a one-run payload override. For configured Codex projects, --command still passes --tool codex so Codex state mounts remain available while the explicit payload script runs.

**2026-05-15T07:53:36Z**

Validation: cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check passed. cargo test --manifest-path vm-frontend/Cargo.toml --offline passed after the main implementation. After a final trim-handling tweak for default_command="codex", cargo test --manifest-path vm-frontend/Cargo.toml --offline configured_ passed.
