---
id: wra-6cei
status: closed
deps: [wra-gx6d]
links: []
created: 2026-05-15T10:39:27Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-neci
tags: [validation, cli, config, ux]
---
# Add CLI config, setup-tool, and command override UX tests

The CLI/TUI contract changed toward project-local config.json as the source of truth, setup-tool recipes, and command overrides after --. Add integration tests around vm-frontend/src/main.rs and related config/setup code. Cover first-run setup, no repeated prompts when config exists, default agentvm behavior, --setup-tool codex and --setup-tool pi recipes, config fields for entrypoint/mounts/network/host allowlist, command override after --, shell session override, precedence between config and CLI switches, invalid config diagnostics, and migration/backward compatibility for old --tool usage if still supported.

## Design

Prefer assert_cmd-style CLI tests against the built binary and temporary project directories. Keep recipe effects testable without network by using command shims for package installation such as miso install. Tests should assert resulting config.json content, process arguments passed to launch, and user-facing diagnostics.

## Acceptance Criteria

CLI tests prove setup recipes write the expected config.json, existing config suppresses setup prompts, -- command override works, invalid config is explained, and old/conflicting switches behave intentionally. The validation workflow includes these tests in the offline required tier.


## Notes

**2026-05-15T11:09:31Z**

Started and surveyed existing wrapper/config tests in vm-frontend/src/main.rs. There is already substantial model-level coverage for setup-tool pi, configured codex project suppressing startup dialog, post-separator command override, config schema network/auth/shares/ports, argv0 wrapper behavior, invalid unconfigured non-TUI diagnostic, and removed bubblewrap flag rejection. Gaps versus ticket acceptance appear to be binary/assert_cmd-style CLI integration, explicit --setup-tool codex config-write assertions, invalid config diagnostics from disk, shell session override coverage, precedence/conflicting switches, and command shim assertions for setup recipes.

**2026-05-15T11:13:09Z**

Added missing disk/model-level CLI config tests in vm-frontend/src/main.rs: --setup-tool codex writes expected config.json shape and launch args, invalid config from disk reports path and reason, CLI network allowlist overrides config no-net and restores published port behavior, shell --command works for unconfigured plain projects, and legacy schema_version=1 config migrates to current codex launch defaults. Targeted tests passed; docs/fmt/guest-services/fast/fuzz-check passed. host-live remains blocked by stale appliance manifest.

**2026-05-15T11:14:47Z**

Added more CLI/config coverage: setup_tool_codex_writes_expected_config_json, invalid_config_from_disk_reports_path_and_reason, cli_network_overrides_take_precedence_over_config_no_net, shell_command_override_allows_unconfigured_plain_project, and legacy_config_from_disk_migrates_to_current_launch_defaults. These cover codex config persistence shape, disk parse diagnostics, no-net/config precedence, shell override on unconfigured plain projects, and schema v1 migration. Offline docs/fmt/guest-services/fast/fuzz-check passed; host-live still blocked by stale appliance artifacts.

**2026-05-15T11:30:00Z**

After appliance rebuild, ./vm-frontend/validate.sh required passed end-to-end. CLI/config tests are included in offline required tier and cover setup config writes, existing config behavior, command overrides, disk invalid diagnostics, legacy migration, and config/CLI precedence. Live self-test ok provides required live evidence.
