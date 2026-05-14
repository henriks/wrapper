---
id: wra-7v3w
status: closed
deps: [wra-njbl]
links: []
created: 2026-05-14T20:39:23Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-zewf
tags: [ui, tui, input, vm-frontend]
---
# Route TUI keyboard input, resize, and signals to the guest

Complete interactive event routing for the TUI guest terminal.

Context:
- Depends on the visible Ratatui viewport from `wra-njbl`.
- Current plain streaming uses host stdin plus `PayloadControlOptions::interactive()` to forward input, selected signals, and SIGWINCH terminal resizes.
- In TUI mode, the Ratatui/crossterm event loop owns input and must translate events into payload input/control frames based on focus and viewport size.

Relevant code:
- TUI module introduced by `wra-njbl`.
- `vm-frontend/src/payload_client.rs` session methods from `wra-ag8h`.
- Existing signal/resize helpers in `payload_client.rs` may need to be reused or adapted.

Required behavior:
- In Guest focus, normal keys and pasted/input bytes are forwarded to the payload PTY.
- Wrapper-reserved key/prefix behavior follows the design ticket.
- Host terminal resize recomputes layout, resizes the terminal emulator model, and sends a payload `W` resize frame for the viewport interior size.
- Interrupt/termination behavior remains sane: intended guest interrupts reach the payload process group, while wrapper shutdown still restores terminal state.

## Design

Keep event routing explicit and testable. Model host UI events separately from payload protocol events so future wrapper prompts can take focus without changing the payload client.

## Acceptance Criteria

- Guest-focused TUI sessions accept keyboard input and forward it to the payload PTY.
- Resize events update both the rendered viewport and guest PTY size.
- Signal/interrupt behavior is documented and matches the design.
- Wrapper-reserved controls do not leak unintended bytes to the guest.
- Tests cover key-event translation and viewport resize calculations where practical.


## Notes

**2026-05-14T21:17:28Z**

Implemented TUI guest input routing in `vm-frontend/src/tui.rs`: payload output is read on a background thread, crossterm key events are translated into guest bytes, paste events are forwarded, Ctrl-C sends SIGINT to the guest payload, Ctrl-\ is reserved for wrapper controls and does not leak to the guest, and resize events recompute the bordered viewport interior, resize the vt100 screen, and send payload resize frames. Added tests for key translation, reserved prefix behavior, and resize math. Verified with `cargo test --manifest-path vm-frontend/Cargo.toml --offline`.
