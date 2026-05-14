---
id: wra-fzao
status: closed
deps: [wra-njbl]
links: []
created: 2026-05-14T20:39:34Z
type: feature
priority: 2
assignee: Henrik Saksela
parent: wra-zewf
tags: [ui, tui, status, vm-frontend]
---
# Add wrapper status bar for VM and payload session state

Add a concise wrapper-owned status bar to the TUI.

Context:
- Depends on the initial Ratatui viewport from `wra-njbl`.
- The status bar should make the wrapper visible without stealing the main terminal area.
- Useful state exists across `vm-frontend/src/main.rs`, `vm-frontend/src/launch.rs`, runtime state written to `.sandbox/docker-vm/run/state.json`, and policy/config objects.

Initial status fields to consider:
- Launch phase: starting, waiting for payload, running, shutting down, exited/error.
- Tool/payload identity: codex/copilot/custom payload where available.
- Network mode: public egress allowed vs `--no-net`.
- VM/runtime hints: qemu pid once known, run dir/log hint, payload exit code on completion.
- Focus mode: guest vs wrapper prompt once prompt support exists.

Keep the bar compact. It should support scanning, not become a dashboard.

## Design

Use a one-line status bar first. Avoid reading/parsing logs in this ticket unless a specific field needs it. Prefer pushing state changes from the launch/session controller into the TUI model.

## Acceptance Criteria

- TUI includes a one-line status bar outside the guest terminal frame.
- Status updates during launch, payload running, exit, and error paths.
- Status text fits at narrow terminal widths through truncation or prioritized fields.
- Plain streaming output is unaffected.
- Tests cover status formatting/truncation where practical.


## Notes

**2026-05-14T21:18:49Z**

Implemented one-line TUI status model in `vm-frontend/src/tui.rs`: status now tracks session phase, focus mode, and guest viewport size, formats into the reserved status row, and truncates to narrow widths. Status updates on starting, running output, resize, exit, and failure. Added formatting/truncation tests. Verified with `cargo test --manifest-path vm-frontend/Cargo.toml --offline`.
