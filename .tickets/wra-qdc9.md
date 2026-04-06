---
id: wra-qdc9
status: in_progress
deps: []
links: []
created: 2026-04-06T11:04:50Z
type: bug
priority: 1
assignee: Henrik Saksela
tags: [wrapper, cli]
---
# Restore -- command replacement semantics in sandbox wrapper

The sandbox wrapper currently prepends the selected tool CLI even when arguments are supplied after --. That broke the original behavior where tokens after -- replace the default tool entry point and are executed directly inside the sandbox. Restore that behavior for pi-wrap, codex-wrap, and copilot-wrap, confirm whether all wrappers were affected, and align requirements.md wording with actual semantics.

