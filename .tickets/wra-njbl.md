---
id: wra-njbl
status: closed
deps: [wra-ag8h, wra-d54r]
links: []
created: 2026-05-14T20:39:09Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-zewf
tags: [ui, tui, ratatui, vm-frontend]
---
# Render guest payload in a Ratatui terminal viewport

Implement the first visible TUI shell: a framed guest terminal viewport using Ratatui.

Context:
- Depends on payload session APIs from `wra-ag8h` and mode selection from `wra-d54r`.
- The guest payload server emits PTY bytes as `O` frames. Those bytes may include ANSI/VT control sequences from Codex, Copilot, shells, editors, and other terminal applications.
- Ratatui handles layout/rendering, but terminal emulation should use `vt100` directly or a Ratatui widget such as `tui-term`.

Relevant code:
- New TUI module likely under `vm-frontend/src/` such as `tui.rs` or `ui/`.
- `vm-frontend/Cargo.toml` for dependencies.
- `vm-frontend/src/main.rs` wrapper launch path for selecting TUI mode.
- `vm-frontend/src/payload_client.rs` lower-level session API.

Required UI:
- Guest terminal frame fills most of the screen.
- Frame interior size determines payload terminal rows/cols.
- The wrapper reserves space for a status bar ticket to fill in later.
- TUI initializes/restores terminal state correctly on normal exit and error.

## Design

Start with a conservative layout: full-screen Ratatui terminal, bordered main guest viewport, one-line reserved status region. Use `vt100` or `tui-term` to maintain guest screen state rather than writing guest bytes directly to stdout. Keep this ticket focused on rendering and lifecycle, not prompt UX.

## Acceptance Criteria

- Interactive wrapper runs can display guest PTY output inside a framed viewport.
- ANSI/VT output is parsed/rendered through a terminal model, not printed outside the frame.
- Payload initial rows/cols use the viewport interior dimensions.
- Terminal raw/alternate-screen state is restored after payload exit, failure, and Ctrl-C/error paths.
- Plain streaming remains unaffected when TUI is disabled.
- Tests or a documented manual smoke cover basic rendering and cleanup behavior.


## Notes

**2026-05-14T21:15:47Z**

Implemented initial Ratatui viewport in `vm-frontend/src/tui.rs` using `tui_term::widget::PseudoTerminal` backed by vt100 parser state. TUI mode now initializes Ratatui, computes payload rows/cols from the bordered viewport interior, renders guest output inside the frame, reserves a one-line status area, and restores terminal state via `ratatui::try_restore` on exit/error. Added viewport layout/render tests. Added `ratatui`, `crossterm`, `tui-term`, and `vt100` dependencies. Verified with `cargo test --manifest-path vm-frontend/Cargo.toml --offline`.
