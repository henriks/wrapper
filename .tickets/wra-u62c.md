---
id: wra-u62c
status: closed
deps: [wra-huvu, wra-tqup]
links: []
created: 2026-05-15T08:19:02Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-293p
---
# Add setup-tool recipes for known agent configurations

Introduce explicit tool setup recipes as the replacement for the current overloaded --tool launch switch. A recipe like --setup-tool codex or --setup-tool pi should intentionally write config.json with the default entrypoint/command, install/bootstrap method, required writable tool-state shares, environment defaults, and known package metadata. Codex should configure Codex as the default entrypoint and preserve the required rw state shares. Pi should support @mariozechner/pi-coding-agent through a recipe name such as pi. This depends on the expanded config.json schema and should align with the agentvm primary UX. Relevant code: WrapperSandboxConfig and parse_wrapper_args_with_terminal in vm-frontend/src/main.rs; GuestTool/tool state handling in vm-frontend/src/runtime_manifest.rs; UX/config tickets wra-huvu, wra-tqup, wra-o8pc.

## Acceptance Criteria

Recipes are versioned/config-driven rather than hard-coded launch flags where practical. Running a setup recipe updates config.json only because the user explicitly requested setup. Normal launch does not persist CLI overrides. Tests cover codex recipe config output, pi recipe config output, launch from recipe-generated config, and compatibility/deprecation behavior for --tool.


## Notes

**2026-05-15T08:34:21Z**

Added explicit --setup-tool recipes for codex and pi. Recipes build durable config.json records with setup_tool, default command, package metadata behavior, and tool state. Codex uses the existing GuestTool path and writable .codex state; Pi installs/launches @mariozechner/pi-coding-agent via the generic npm bootstrap and enables writable .pi tool state. Normal launch overrides do not persist; --setup-tool intentionally writes config before launch. Tests cover Pi recipe config/launch output and configured Codex behavior.
