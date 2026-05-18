---
id: wra-8fjd
status: open
deps: [wra-3s2f]
links: [wra-m7gg, wra-ylfx, wra-d6vo, wra-2qij, wra-3s2f]
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

**2026-05-18T10:37:46Z**

Follow-up cleanup epic wra-emj5 adds wra-3s2f as a prerequisite. Do not solve TUI post-restore failures by adding another wrapper outcome path around the existing blocking payload client. First collapse plain/TUI payload execution onto one session architecture and shared structured outcome, then print the post-restore summary from that outcome.

**2026-05-18T10:55:02Z**

Context from wra-3s2f audit: payload_client.rs already has PayloadSessionOutcome::{Exit,Failure,Cancelled}. wra-3s2f should expose/return this structured outcome (or a direct replacement) from both plain launch and TUI so this ticket can print failures after terminal restore without re-parsing PayloadClientError or adding a TUI-only adapter.

**2026-05-18T11:01:33Z**

wra-3s2f iteration 6 changed TUI to return PayloadSessionOutcome through launch_cli after terminal restore. This should make this ticket straightforward: print/report PayloadSessionOutcome::Failure after ratatui restore rather than adding a separate TUI error channel.
