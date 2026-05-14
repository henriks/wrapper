---
id: wra-ukjo
status: open
deps: [wra-jndb, wra-lf5f]
links: []
created: 2026-05-14T20:40:14Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-zewf
tags: [ui, tui, initialization, vm-frontend]
---
# Add TUI sandbox initialization dialog

Add an initial TUI dialog for starting/configuring a new sandbox session.

Context:
- Epic: `wra-zewf` Interactive TUI wrapper for VM payload sessions.
- Direction update: do not rely on `codex-wrap` / `copilot-wrap` argv[0] entrypoint inference long term. Starting a sandbox should have an explicit path, and the default-on TUI can ask what should be initialized.
- This ticket depends on the prompt/focus infrastructure from `wra-jndb`.

The dialog should collect or confirm the initial session choices that are currently implicit or CLI-only, such as:
- Which tool/payload to initialize/run, if more than one remains supported.
- Project path / sandbox root when not already explicit.
- Network mode, including the equivalent of `--no-net`.
- Optional auth/state integrations such as GitHub or AWS profile, matching existing supported wrapper flags.
- Whether to use existing sandbox state or reset/reinitialize when relevant.

Relevant code:
- `vm-frontend/src/main.rs`: wrapper argument parsing and launch argument construction.
- TUI modules introduced by earlier tickets.
- Existing runtime/state layout in `requirements.md` and `.sandbox/docker-vm/run/state.json` writer.

## Design

Treat this as a startup flow/wizard that produces the same kind of launch configuration the CLI currently builds. Keep CLI flags usable for automation, but make the TUI capable of asking for missing interactive choices. Avoid making the dialog tool-specific if a generic payload/tool selection model is enough.

## Acceptance Criteria

- Starting a TUI sandbox can present an initialization dialog before launching the guest payload.
- The dialog produces structured launch/session configuration rather than mutating argv strings directly where avoidable.
- Existing explicit CLI flags can pre-fill or bypass corresponding dialog choices.
- The dialog has keyboard navigation and accept/cancel behavior consistent with the prompt focus model.
- Plain/non-TTY mode remains scriptable and does not require interactive dialogs.
- Tests cover configuration produced by dialog choices where practical.


## Notes

**2026-05-14T20:40:55Z**

Superseded by later clarification: initialization UI should not present `codex-wrap` / `copilot-wrap` as argv[0]-based entrypoints. The relevant setup concern is Codex: enabling Codex in the TUI should add the correct Codex tool-state/config directories as rw mounts in the sandbox config.
