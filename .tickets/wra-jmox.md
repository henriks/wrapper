---
id: wra-jmox
status: closed
deps: []
links: []
created: 2026-05-14T21:37:41Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-zewf
tags: [ui, tui, vm-frontend]
---
# Remove TUI terminal frame

Adjust the wrapper TUI so the guest terminal renders without a border/frame. Keep the bottom wrapper status/prompt line. Update viewport sizing so payload rows/cols use the full terminal area above the status bar rather than subtracting border dimensions.

## Acceptance Criteria

- Guest terminal viewport renders without a Ratatui border/block.
- Status/prompt bar remains at the bottom.
- Payload rows/cols use the full viewport area above the status line.
- Tests are updated for the new sizing behavior.
- Build/tests pass.


## Notes

**2026-05-14T21:49:27Z**

Implemented unframed TUI terminal rendering. Guest PTY sizing now uses the full viewport above the status bar, so no border rows/columns are subtracted. Updated TUI sizing tests and validation/design docs. Verified with cargo fmt, cargo test --manifest-path vm-frontend/Cargo.toml --offline tui::tests, and cargo build --manifest-path vm-frontend/Cargo.toml --offline.
