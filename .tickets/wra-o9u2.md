---
id: wra-o9u2
status: closed
deps: []
links: []
created: 2026-05-15T08:56:53Z
type: task
priority: 4
assignee: Henrik Saksela
parent: wra-sne0
---
# Make TUI status truncation display-width aware

Replace manual char-count truncation in vm-frontend/src/tui.rs truncate_status with a display-width-aware implementation. Candidate crates: unicode-width, optionally textwrap. Current code uses chars().count and can mis-size wide Unicode or combining marks in status/prompt text.

## Acceptance Criteria

Status/prompt truncation accounts for Unicode display width or a note justifies ASCII-only status text. Unit tests cover narrow width and at least one wide Unicode case. cargo test --manifest-path vm-frontend/Cargo.toml --offline passes.


## Notes

**2026-05-15T09:19:38Z**

truncate_status now uses unicode-width for display-width measurement and a width-aware take_display_width helper. Added wide Unicode test coverage for fitting and truncating prompt/status text. Validation: vm-frontend offline tests and fmt check pass.
