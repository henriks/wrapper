---
id: wra-xws9
status: closed
deps: []
links: []
created: 2026-05-15T08:56:48Z
type: task
priority: 3
assignee: Henrik Saksela
parent: wra-sne0
---
# Use TUI input helper crates for config editor fields

Replace or reduce bespoke config editor/startup dialog input handling in vm-frontend/src/tui.rs with focused TUI input helpers. Candidate crates: tui-input or tui-textarea for Ratatui-native text fields; inquire/dialoguer only if setup/config flows are intentionally moved outside the full-screen TUI. Current code has custom dialog/config editor state machines, key matching, toggles, centered modal rendering, and static help text.

## Acceptance Criteria

At least one real editable config field uses a maintained input widget/helper instead of bespoke char/backspace handling, or a note documents why current custom handling is preferable. The TUI still writes config.json only on explicit save. Existing TUI unit tests pass and any new model tests cover edit/cancel/save behavior.


## Notes

**2026-05-15T09:19:30Z**

Prompt text input in the Ratatui wrapper now uses tui-input's crossterm EventHandler instead of bespoke char/backspace mutation. The current config editor itself is still mostly toggle/cycle controls, so there was no real text field there to migrate yet. Existing prompt/edit/save/cancel model tests pass.
