---
id: wra-5rrw
status: open
deps: [wra-8xjs]
links: []
created: 2026-05-15T21:53:19Z
type: task
priority: 1
assignee: Henrik Saksela
tags: [tests, live, pi, mise, setup-tool]
---
# Add Pi/mise setup-tool live smoke after mise bootstrap

Follow-up split from wra-13x7. Codex setup-tool bootstrap/persistence is now covered by required live-setup-tools via wra-j7lp. After wra-8xjs restores mise-based agent tool bootstrap, add/extend live setup-tool validation for Pi and assert the intended mise path rather than the temporary npm-only path.

## Acceptance Criteria

live-setup-tools (or a named sibling tier) covers Pi setup-tool bootstrap non-interactively with a bounded version/help command. The test verifies the intended mise bootstrap path after wra-8xjs, survives payload shutdown/relaunch as applicable, and documentation names when it runs. Required gate policy is updated if Pi should become mandatory.

