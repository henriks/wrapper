# Wrapper TUI Design

## Scope

The wrapper should start interactive VM payload sessions in a terminal UI by
default. The guest payload remains the real interactive terminal, but the host
wrapper owns the screen, a status bar, and future prompts that can temporarily
take focus away from the guest.

This design applies to the wrapper/startup path, not to the low-level
`payload-client` debugging command. Non-interactive and scripted use must keep a
plain streaming path.

## Current Boundaries

Important existing files:

- `vm-frontend/src/main.rs`
  - `run_cli` uses explicit frontend subcommands; the legacy `codex-wrap` /
    `copilot-wrap` executable-name entrypoints have been removed.
  - `run_wrapper` parses wrapper flags and delegates to `launch`.
  - `launch` starts the VM, waits for the payload listener, then calls
    `run_payload_tcp_with_control` with host stdin/stdout.
  - `runtime_mounts`, `guest_payload_env`, and `launch_payload_args` translate
    selected setup/config state into runtime mounts and payload scripts.
- `vm-frontend/src/payload_client.rs`
  - Implements the framed payload protocol and direct stdin/stdout forwarding.
  - Already supports payload input (`I`), output (`O`), resize (`W`), signals
    (`S`), exit (`X`), and failure (`F`).
- `docker/guest-payload-server.py`
  - Runs the payload under a guest PTY and applies rows/cols from the request.
- `vm-frontend/src/runtime_manifest.rs`
  - Maps selected tool state into composed filesystem mounts.

The TUI should be introduced as a new owner of host terminal input/output. It
should not be implemented by writing guest bytes directly to stdout inside a
Ratatui frame.

## CLI Behavior

Interactive wrapper startup uses the TUI by default. Add `--no-tui` as the
explicit opt-out flag for wrapper/startup mode. `--no-tui` should select the
existing plain streaming behavior.

Mode selection:

- `Auto`: the default for wrapper/startup commands.
- `Tui`: selected by `Auto` only when both stdin and stdout are TTYs.
- `Plain`: selected by `--no-tui`, or automatically when stdin/stdout are not
  TTYs.

The automatic non-TTY fallback is required so CI, shell pipelines, and scripted
payload launches do not enter raw mode or require dialog interaction.

The user-facing wrapper path is `agentvm`. `agentvm-frontend wrap` remains a
development path. Tool setup comes from `--setup-tool` recipes, structured
startup configuration, or the TUI initialization dialog; legacy
`codex-wrap`/`copilot-wrap` executable-name inference remains unsupported.

## Dependencies

Use:

- `ratatui` with the `crossterm` backend for layout, rendering, raw mode, and
  keyboard/resize events.
- `vt100` directly, or `tui-term` if it fits the viewport rendering better, for
  guest PTY terminal emulation.

Ratatui is a widget/layout library. The guest viewport must be backed by a
terminal emulator model so ANSI/VT output from Codex, shells, editors, and other
tools is interpreted inside the frame.

Keep Ratatui dependencies out of `payload_client.rs`. The payload protocol
module should expose transport/session primitives that can be reused by both
plain streaming and the TUI.

## Runtime Architecture

Separate the implementation into four layers:

1. `PayloadSession`
   - Owns the TCP stream to the guest payload server.
   - Sends the initial `PayloadRequest`.
   - Receives payload events: output bytes, exit code, and failure.
   - Sends input bytes, resize frames, and signal frames.
   - Does not know about terminal UI libraries.
2. Launch/session controller
   - Starts the VM with `start_frontend_with_policy`.
   - Waits for the payload listener.
   - Builds the payload request from selected startup configuration.
   - Terminates the running frontend after payload exit or wrapper cancellation.
   - Publishes status transitions to the TUI model.
3. TUI model
   - Stores focus mode, status state, prompt state, viewport dimensions, and the
     terminal emulator screen.
   - Converts host UI events into actions.
   - Converts payload events into terminal model updates and exit/error status.
4. Ratatui renderer
   - Draws the unframed guest terminal viewport.
   - Draws the status bar.
   - Draws wrapper prompts/dialogs when active.

Use channels between the TUI event loop and background payload/session work so
the UI can continue rendering while the VM starts and while guest output arrives.

## Layout And Sizing

The default layout is:

- Main area: unframed guest terminal viewport.
- Bottom line: wrapper status bar.
- Optional prompt/dialog overlay or bottom prompt area when wrapper focus is
  active.

The guest PTY size must be the terminal viewport above the status bar, not the host terminal size. Subtract any status or prompt space before sending rows/cols to the guest.

Sizing rules:

- Clamp rows and columns to at least 1 before sending a payload resize.
- Initial `PayloadRequest.rows` and `PayloadRequest.cols` use the viewport
  dimensions computed on first render.
- Host resize events recompute layout, resize the terminal emulator model, and
  send a `W` frame with the new viewport size.
- Avoid layout shifts from status text; status fields should truncate by
  priority at narrow widths.

## Focus And Input

Model focus explicitly:

- `Guest`: default during an active payload session. Text input, paste, enter,
  tab, arrows, and normal control sequences are translated to bytes and sent as
  payload `I` frames.
- `WrapperPrompt`: wrapper-owned prompt or startup dialog has focus. Normal
  typed keys edit/select prompt state and must not be sent to the guest.
- `WrapperCommand`: reserved future mode for command palette/help/log views.
- `Exiting`: payload exited or wrapper is shutting down; input is limited to
  acknowledgement/cleanup actions.

Reserve a wrapper control prefix rather than many global shortcuts. The design
should default to a low-collision prefix such as `Ctrl-\` followed by a command
key. The exact key mapping can be adjusted during implementation, but it must be
centralized and testable.

Interrupt behavior:

- In `Guest` focus, `Ctrl-C` should behave like guest input/signal behavior
  expected by the payload. The existing signal-forwarding semantics should be
  preserved as closely as possible.
- In wrapper focus, cancel keys should affect the prompt first.
- Wrapper process termination must always restore raw mode/alternate screen.

## Status Bar

The first status bar should be one line. Candidate fields, in priority order:

1. Launch/session state: starting, waiting for payload, running, prompt, exiting,
   exited, error.
2. Focus mode when not `Guest`.
3. Network policy: public egress or `--no-net`.
4. Selected payload/tool, such as Codex, when known.
5. QEMU pid and run directory/log hint when space allows.
6. Payload exit code after completion.

Status should come from controller state changes, not by tailing logs.

## Startup Dialog And Config Editing

The TUI startup dialog should produce structured sandbox configuration. CLI
flags can pre-fill or bypass choices, but the dialog is the interactive source
for missing startup decisions.

Config editor and startup choices:

- Setup recipe: Codex, Pi, or custom command.
- Project path when not supplied.
- Network mode: public, none, or allowlist.
- Whitelisted domains/hosts/IPs for allowlist mode.
- Optional GitHub auth sharing (`--gh`).
- Optional AWS profile (`--aws PROFILE`).
- Additional `--ro` and `--rw` shares.
- Published guest ports.
- Reset/reinitialize sandbox state when requested.

Enabling a setup recipe must add the correct tool state/config directories as rw
mounts in `.sandbox/config.json`. This should be represented as explicit
structured configuration consumed by `guest_runtime_mounts`/runtime manifest
generation, not as executable-name inference from `codex-wrap`.

## Implementation Order

1. Add CLI mode selection and `--no-tui` plumbing while preserving plain
   streaming.
2. Refactor `payload_client.rs` to expose reusable session primitives.
3. Add the Ratatui viewport and terminal emulator rendering.
4. Route keyboard input, resize, and signal/control events through the TUI.
5. Add the status bar.
6. Add wrapper prompt focus.
7. Add the startup dialog and Codex mount configuration.
8. Keep documentation and tests aligned with explicit wrapper startup, not
   executable-name aliases.
9. Add validation coverage and an interactive smoke workflow.

This order keeps a working plain path available until the TUI replacement can
start a configured sandbox without relying on executable names.

## Validation Plan

Fast offline tests should cover:

- Wrapper mode selection: default auto, `--no-tui`, and non-TTY fallback where
  practical.
- Payload session frame send/receive behavior.
- Viewport layout math and rows/cols clamping.
- Key-event to guest-byte translation.
- Focus routing: prompt input does not leak to the guest.
- Status formatting/truncation.
- Codex mount configuration generated from the startup choices.
- `argv[0]` no longer selects tool/wrapper behavior.

Manual live smoke should cover:

1. Start an interactive sandbox and confirm the TUI appears by default.
2. Configure Codex or Pi and verify tool state dirs are present as rw mounts in
   the generated composed-fs manifest.
3. Type into the guest terminal and confirm guest output stays inside the frame.
4. Resize the host terminal and confirm the guest PTY observes the viewport
   size.
5. Open a wrapper prompt and confirm typed input does not reach the guest.
6. Run with `--no-tui` and confirm plain streaming behavior.
7. Interrupt and normal-exit sessions and confirm terminal state is restored.
