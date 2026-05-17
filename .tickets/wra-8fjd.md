---
id: wra-8fjd
status: open
deps: []
links: [wra-m7gg, wra-ylfx, wra-d6vo, wra-2qij]
created: 2026-05-16T15:50:47Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [frontend, tui, ux]
---
# Print TUI payload failures after terminal restore

Problem:
The TUI renders nonzero exits or protocol errors inside the alternate screen, then restores the terminal. run_launch then exits without a stable post-restore failure message.

Relevant code:
- vm-frontend/src/tui.rs:655, 702, and 707 render/return session outcomes.
- vm-frontend/src/main.rs:180-201 handles payload result and process exit.

Impact:
Users may see only a shell exit code after terminal restore, with no visible failure reason.

Recommended fix:
Return a structured TUI outcome and print a concise post-restore summary to stderr for nonzero exits and protocol failures before process exit.

Validation:
- PTY/TUI test for a payload that exits nonzero, asserting restored terminal output contains a stable failure summary.


## Notes

**2026-05-16T21:46:46Z**

Review after wra-35eb/wra-m7gg: still valid. Plain payload mode now has PayloadSessionOutcome and cancellable runner semantics, but TUI still runs run_payload_viewport -> Result<i32, PayloadClientError> and main.rs maps the error to a string after terminal restore without a structured post-restore failure outcome. This ticket should use the new outcome/policy vocabulary where useful, but it still owns stable stderr summaries for TUI nonzero/protocol/cancel failures.
