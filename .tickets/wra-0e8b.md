---
id: wra-0e8b
status: closed
deps: []
links: []
created: 2026-04-06T10:54:57Z
type: bug
priority: 1
assignee: Henrik Saksela
tags: [wrapper, pi, tui]
---
# Fix terminal resize propagation for pi in sandbox wrapper

Investigate why terminal resize events (SIGWINCH) do not reach the pi TUI when launched via /home/hsaksela/ai/wrapper/sandbox-wrap or pi-wrap. Outside the wrapper, pi rerenders correctly on terminal resize. Determine whether Bubblewrap session/process-group handling is preventing resize delivery, reproduce with a small interactive test if possible, and update the wrapper so interactive pi gets resize events while preserving existing behavior for other tools as much as possible. Document the rationale in requirements.md if behavior changes.


## Notes

**2026-04-06T10:58:42Z**

Root cause: bubblewrap --new-session suppresses terminal-generated SIGWINCH delivery to child TUIs. I reproduced this with a PTY-backed Python harness: plain child and bwrap without --new-session both received WINCH after TIOCSWINSZ, while bwrap with --new-session did not. Fix: only add --new-session for non-interactive invocations; when stdin/stdout/stderr are TTYs, keep the caller terminal session so pi and other interactive TUIs continue to redraw on resize.
