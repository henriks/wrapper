---
id: wra-ntek
status: closed
deps: [wra-6cei, wra-l8ke]
links: []
created: 2026-05-15T10:39:35Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-neci
tags: [validation, tui, ux, config]
---
# Add TUI interaction tests for config editing and launch flows

The TUI should be coherent and convenient, centered on the 80% agentvm flow while exposing relevant config.json editing over time. Add terminal-level tests for the wrapper UI using the existing TUI patterns/tools. Cover configured project startup without reprompting, setup flow for unconfigured projects, editing entrypoint/mount/network/allowlist fields, command override visibility, launch/cancel/error flows, focus order, resize behavior, and clear diagnostics when config or appliance validation fails.

## Design

Use a real terminal harness such as tmux/pty snapshots rather than only model-level unit tests for layout and focus behavior. Keep snapshots stable by fixing terminal size and avoiding brittle cosmetic assertions. Assert no in-app text overlaps and no prompt appears when config.json is already valid.

## Acceptance Criteria

TUI tests cover first-run setup, configured startup, config editing, launch failure diagnostics, keyboard navigation, and resize at representative terminal sizes. The tests are documented and runnable locally without live VM boot unless explicitly marked live.


## Notes

**2026-05-15T14:50:47Z**

Started with a first terminal-level harness slice using util-linux script(1) as a pty wrapper around the real agentvm binary. Added vm-frontend/tests/tui_terminal.rs with scripted-key tests for first-run startup cancel diagnostics (no config persisted on 'n') and config editor save flow (keys c/n/g/d/p/s update command, network, GitHub auth, share, and published-port fields in .sandbox/config.json). Targeted validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline --test tui_terminal -- --nocapture. Remaining acceptance gaps: configured startup without reprompt, launch failure diagnostics from a terminal run, focus/navigation/resize snapshots at fixed sizes, and documentation for running the TUI tests.

**2026-05-15T14:54:00Z**

Expanded the pty-backed TUI harness. It now bounds no-input launch runs with timeout(1), writes a valid codex config, and covers configured startup without a first-run reprompt by forcing an invalid qemu path and asserting stable launch phase/artifact diagnostics instead of booting a VM. Also documented the automated pty tests in vm-frontend/validation-workflow.md. Targeted validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline --test tui_terminal -- --nocapture (3 tests).

**2026-05-15T14:56:19Z**

Added deterministic TUI layout/focus coverage in vm-frontend/src/tui.rs using ratatui TestBackend: startup dialog renders at 80x24 and 42x12, config editor renders edited command/network/auth/share/published-port summary at representative sizes, and wrapper prompt focus consumes typed/control keys without leaking them as guest input before returning focus on Enter. Targeted validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline tui::tests:: -- --nocapture (16 TUI tests in both bin targets).

**2026-05-15T14:57:20Z**

Final validation for TUI ticket passed after pty and TestBackend coverage: cargo test --manifest-path vm-frontend/Cargo.toml --offline; ./vm-frontend/validate.sh required; and ./vm-frontend/validate.sh live. Required included docs/fmt, composed-fs/vm-frontend offline tests with tui_terminal integration tests, guest-services, fuzz-check, and live-smoke self-test reaching self-test: ok. Acceptance is satisfied: first-run setup/cancel, configured no-reprompt startup, config editing, launch failure diagnostics/artifacts, keyboard prompt focus, and representative fixed-size layout/resize seams are covered without requiring ad hoc live scripts.
