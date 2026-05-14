---
id: wra-3vmi
status: closed
deps: [wra-70mh]
links: []
created: 2026-05-14T20:40:35Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-zewf
tags: [ui, tui, validation, vm-frontend]
---
# Validate TUI wrapper behavior and document smoke workflow

Add validation coverage and a manual smoke workflow for the interactive TUI wrapper.

Context:
- Epic: `wra-zewf` Interactive TUI wrapper for VM payload sessions.
- Existing validation docs include `vm-frontend/validation-workflow.md`, `vm-frontend/validation-matrix.md`, and `vm-frontend/validate.sh`.
- Existing non-KVM suite is `cargo test --manifest-path vm-frontend/Cargo.toml --offline`.
- Real KVM behavior matters because the TUI wraps the actual VM payload PTY and launch lifecycle.

Coverage targets:
- Mode selection: default TUI for interactive sessions, explicit TUI opt-out, non-TTY plain fallback.
- Layout/sizing: payload rows/cols are viewport interior dimensions.
- Event routing: guest focus sends input to payload, prompt focus does not.
- Resize: host terminal resize updates UI model and guest PTY.
- Cleanup: terminal raw/alternate-screen state is restored after normal exit, payload error, and interrupted shutdown.
- Docs: manual KVM smoke for starting a TUI sandbox, interacting with the payload, using the initialization dialog, resizing, opting out of TUI, and locating logs.

## Design

Prefer unit tests for pure state machines, mode selection, layout math, status formatting, and event translation. Use a documented real-VM smoke procedure for behavior that needs KVM/PTY integration. If snapshot/golden rendering tests are useful, keep them stable and small.

## Acceptance Criteria

- Non-KVM tests cover the core TUI state machines and CLI mode selection.
- Validation docs describe a real KVM interactive smoke workflow.
- The smoke workflow covers TUI default behavior, opt-out/plain streaming, initialization dialog, guest input/output, resize, and cleanup.
- Existing validation docs or scripts are updated where appropriate.
- Any known terminal compatibility limitations are documented.


## Notes

**2026-05-14T21:25:12Z**

Added interactive TUI smoke workflow to `vm-frontend/validation-workflow.md` covering default TUI startup, Codex initialization dialog, writable Codex state mount assertion, guest input/output, wrapper prompt focus isolation, resize behavior, `--no-tui` plain fallback, cleanup, and artifact collection. Added Wrapper TUI row to `vm-frontend/validation-matrix.md`. Verified non-KVM coverage with `vm-frontend/validate.sh fast`.
