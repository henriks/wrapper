---
id: wra-lasx
status: in_progress
deps: []
links: []
created: 2026-05-18T20:50:50Z
type: task
priority: 2
assignee: Henrik Saksela
tags: [tui, payload, ux]
---
# Coalesce TUI payload rendering to reduce terminal flicker

The agentvm TUI currently redraws the ratatui/vt100 viewport after every PayloadEvent::Output chunk in vm-frontend/src/tui.rs::run_payload_viewport. Interactive/full-screen CLIs such as Codex/Pi can emit a logical frame as several PTY/TCP chunks (clear/move cursor, partial redraw, final content), so the wrapper renders intermediate terminal states and the console appears to flicker/tear. We discussed that true remote vsync/frame-boundary sync is not available over a plain terminal byte stream, but coalescing output before drawing should reduce visible flicker.

## Design

Restructure run_payload_viewport to keep feeding all payload bytes to GuestTerminalView immediately, but decouple rendering from individual output packets. Maintain a dirty flag and render on a bounded tick (for example 16-33ms / 30-60 FPS). Prefer also draining immediately-ready payload events before a draw. Input forwarding and resize handling must remain immediate; resize should still send guest RESIZE promptly. Exit/failure paths must force a final draw before returning. Consider a tiny debounce/drain window if a simple tick still exposes clear-then-redraw bursts, but avoid noticeable input/output latency.

## Acceptance Criteria

Codex/Pi-style full-screen payload output no longer redraws once per output packet; TUI rendering is capped/coalesced while preserving immediate input and resize behavior. Existing TUI tests pass, and add focused regression/unit coverage around dirty/tick rendering or event-loop behavior where practical. Required validation should be run after implementation; if live validation cannot run, document the limitation.


## Notes

**2026-05-18T20:52:53Z**

Implemented TUI render coalescing in vm-frontend/src/tui.rs. run_payload_viewport now feeds output into vt100 immediately but only redraws on a 16ms render tick when dirty; prompt/resize redraws are also coalesced, while input forwarding and resize control messages remain immediate. Exit/failure still force a final draw. Added RenderCoalescer unit coverage for batching multiple dirty events into one tick. Offline vm-frontend validation-self-test suite passes. Full required validation was not rerun to completion because appliance artifacts are currently stale from docker/guest-init.sh changes and make release-appliance requires sudo/TTY in this harness.
