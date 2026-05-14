---
id: wra-jndb
status: open
deps: [wra-7v3w, wra-fzao]
links: []
created: 2026-05-14T20:39:44Z
type: feature
priority: 2
assignee: Henrik Saksela
parent: wra-zewf
tags: [ui, tui, prompt, vm-frontend]
---
# Add wrapper-owned prompt focus mode for future decisions

Implement the first wrapper-owned prompt/focus path so future wrapper decisions can request input without sending keys to the guest payload.

Context:
- Depends on event routing from `wra-7v3w` and the status model from `wra-fzao`.
- Product direction: the wrapper will later need to prompt the user for decisions while a VM payload session is active.
- This ticket should provide the UI/focus infrastructure and at least one simple internal/demo prompt path, without needing to implement the future policy decisions themselves.

Relevant concepts:
- Focus modes from the design ticket: Guest and WrapperPrompt at minimum.
- TUI event loop owns keyboard input and decides whether events become payload `I` frames or wrapper UI edits/actions.
- Status bar should indicate focus mode.

Possible prompt behavior:
- Modal or bottom prompt area that can ask a short question and accept/select a response.
- Keyboard handling for accept/cancel.
- Guest terminal keeps rendering while prompt is active, but guest input is paused unless explicitly allowed by the design.

## Design

Keep prompt state generic enough for future callers: prompt text, choices or freeform input, default action, result channel/callback, and cancellation semantics. Avoid hard-coding a specific future decision policy.

## Acceptance Criteria

- TUI has a WrapperPrompt focus mode distinct from Guest focus.
- While a prompt has focus, normal typed keys do not go to the guest payload.
- Prompt accept/cancel produces a structured result for the wrapper/controller.
- Guest output continues to render while the prompt is visible unless the design documents otherwise.
- Status/focus indication reflects prompt mode.
- Tests cover focus switching and input routing.

