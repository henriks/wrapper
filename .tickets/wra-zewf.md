---
id: wra-zewf
status: closed
deps: []
links: []
created: 2026-05-14T20:38:19Z
type: epic
priority: 1
assignee: Henrik Saksela
tags: [ui, tui, vm-frontend]
---
# Interactive TUI wrapper for VM payload sessions

Build a default-on terminal UI for wrapper-mode VM payload sessions.

Context:
- The supported runtime is the Rust `agentvm-frontend` in `vm-frontend/`.
- Wrapper entrypoints (`codex-wrap`, `copilot-wrap`, and `agentvm-frontend wrap`) currently route into `run_wrapper` in `vm-frontend/src/main.rs`, then launch the VM and stream payload stdio directly with `run_payload_tcp_with_control`.
- The guest payload server (`docker/guest-payload-server.py`) already runs the payload under a PTY and supports framed input (`I`), output (`O`), resize (`W`), signal (`S`), exit (`X`), and failure (`F`) frames.
- The new UX should make the TUI the default for wrapper-mode interactive sessions, with an explicit flag to disable it and preserve plain streaming.

Product direction:
- The wrapped VM terminal should fill most of the host terminal in a framed viewport.
- The wrapper owns a status bar for launch/runtime/network/exit information.
- The design must leave room for future wrapper-owned prompts and decisions that temporarily take input focus away from the guest terminal.
- Non-interactive/non-TTY use must remain usable through plain streaming or an automatic fallback.

Suggested stack:
- `ratatui` with the default/common `crossterm` backend for TUI rendering and event input.
- `vt100` directly or through `tui-term` for parsing/rendering guest PTY output inside the Ratatui viewport.

Implementation should preserve the existing VM launch, payload framing, resize, and signal behavior while introducing a UI event loop that owns terminal input/output.

## Design

Architect around separate responsibilities:
- Launch/runtime supervision stays in the existing frontend launch path.
- Payload protocol handling becomes reusable by both plain streaming and TUI modes.
- The TUI event loop owns the host terminal, renders wrapper UI, parses guest output into a terminal model, and routes input based on focus mode.
- Focus modes should include at least Guest and WrapperPrompt, with room for command/help/log views later.

Default behavior:
- Wrapper-mode interactive sessions use TUI by default.
- Add an explicit opt-out flag such as `--no-tui` or `--plain` after choosing the final CLI naming.
- Automatically fall back to plain streaming when stdin/stdout are not TTYs unless there is a strong documented reason to fail instead.

## Acceptance Criteria

- Wrapper-mode TUI plan is split into implementation tickets with dependencies matching implementation order.
- The final implementation defaults to TUI for interactive wrapper runs and provides an explicit opt-out flag.
- Plain streaming remains available for non-TTY/automation use.
- The guest terminal frame, status bar, resize behavior, and future prompt focus model are represented in the implementation plan.
- Validation covers non-KVM logic and includes a real KVM smoke procedure for interactive behavior.


## Notes

**2026-05-14T20:42:06Z**

Direction updates from planning discussion: TUI should be default with an explicit opt-out/plain flag. Long-term plan should remove `codex-wrap` and `copilot-wrap` argv[0] entrypoint logic completely. Enabling Codex in the TUI should add the correct Codex state/config directories as rw mounts in the sandbox config.

**2026-05-14T21:25:32Z**

Completed implementation tickets. Final state: interactive wrapper runs default to TUI when stdin/stdout are TTYs; `--no-tui` and non-TTY runs use plain streaming; guest output renders through `tui-term`; guest input/paste/resize/Ctrl-C route through the TUI; wrapper prompt focus is available via Ctrl-\ then p; startup dialog can enable Codex and feeds structured launch configuration; Codex state mounts are explicit; `codex-wrap`/`copilot-wrap` argv0 entrypoints and symlinks were removed; validation docs include a TUI smoke workflow. Fast validation passed with `vm-frontend/validate.sh fast`.
