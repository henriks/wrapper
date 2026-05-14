---
id: wra-jmox
status: in_progress
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

