---
id: wra-0bmi
status: closed
deps: []
links: []
created: 2026-05-14T20:38:35Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-zewf
tags: [ui, tui, design, vm-frontend]
---
# Design default-on TUI wrapper architecture and focus model

Write the implementation design for the wrapper TUI before code changes.

Context:
- Epic: `wra-zewf` Interactive TUI wrapper for VM payload sessions.
- Current wrapper path is in `vm-frontend/src/main.rs`: `run_wrapper` builds `launch` args and eventually streams payload stdio via `run_payload_tcp_with_control`.
- Current payload protocol is in `vm-frontend/src/payload_client.rs` and `docker/guest-payload-server.py`. It already supports PTY output/input, terminal resize, signal forwarding, exit, and failure frames.
- Current launch supervision is in `vm-frontend/src/launch.rs`. QEMU stdout/stderr are logged to `qemu.log`; payload PTY output is what users currently see directly on stdout.

Decisions to capture:
- TUI is default for interactive wrapper-mode sessions.
- Add an explicit opt-out flag for plain streaming; decide the final flag name (`--no-tui`, `--plain`, or equivalent) and where it is accepted in wrapper argument parsing.
- Non-TTY stdin/stdout should use plain streaming automatically unless the design documents a stronger reason to fail.
- Main layout: framed guest terminal viewport filling most of the screen plus a wrapper-owned status bar.
- Focus model: at minimum Guest focus and WrapperPrompt focus; leave room for future wrapper command/help/log views.
- Reserved wrapper control key/prefix strategy should minimize collisions with wrapped tools.
- Terminal sizing: payload rows/cols should be based on the framed viewport interior, not the full host terminal size.

## Design

Produce a design note in the repo, probably under `vm-frontend/` or `docker/` alongside existing validation/design docs. Keep it concrete enough that later tickets can implement without reopening major product questions.

## Acceptance Criteria

- A checked-in design note documents architecture, dependencies, event loop responsibilities, focus modes, sizing rules, opt-out flag behavior, non-TTY fallback, and validation plan.
- The note explicitly references the existing files/functions that will change.
- The note chooses Ratatui/crossterm plus vt100 or tui-term, or documents any deviation with rationale.
- The ticket has notes for any implementation-order changes discovered while writing the design.


## Notes

**2026-05-14T20:39:59Z**

Direction update from planning discussion: do not preserve `codex-wrap` / `copilot-wrap` program-name entrypoint logic as a long-term interface. The design should plan an explicit sandbox start flow where the default-on TUI can ask what should be initialized instead of inferring the tool from argv[0]. Include migration/removal order in the design.

**2026-05-14T20:40:52Z**

Superseded by later clarification: do not design `codex-wrap` / `copilot-wrap` argv[0] behavior as the long-term interface. The important setup path is Codex: enabling Codex in the TUI should add the correct Codex directories as rw mounts/config choices without depending on executable-name inference.

**2026-05-14T21:03:27Z**

Added `vm-frontend/tui-design.md`. Key implementation decision: introduce TUI as a new host terminal owner around a reusable payload session API; keep `payload_client.rs` free of Ratatui dependencies; use `--no-tui` as the explicit opt-out; compute guest rows/cols from the framed viewport interior; move Codex setup to structured startup config and remove argv0 tool inference only after the startup dialog exists.
