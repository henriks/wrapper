---
id: wra-jkeg
status: open
deps: [wra-cvmy, wra-n0fe]
links: [wra-zqci, wra-xcvq, wra-n0fe, wra-yl7i]
created: 2026-05-17T10:19:29Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, payload, tui]
---
# Option 1: rewrite payload client and TUI bridge around async sessions

Rewrite payload execution away from cloned blocking TcpStream, stdin/signal threads, Arc<Mutex<TcpStream>>, read deadlines, and TUI polling threads. Use the shared payload protocol and Tokio session ownership.

## Design

Use tokio::net::TcpStream with split read/write halves, one writer owner, bounded mpsc channels for stdin/control frames, tokio::signal::unix for SIGINT/SIGTERM/SIGHUP/SIGWINCH, tokio::time::timeout for diagnostic deadlines, and cancellation integration with the launch supervisor. Keep ratatui rendering synchronous, but bridge terminal events and payload events through async channels.

## Acceptance Criteria

Payload client supports output, input, exit/failure, ping, diagnostic timeout, resize, signal forwarding, cancellation, and TUI viewport behavior without cloned writers or global signal pipe. Async tests cover partial frames, backpressure, cancellation, and deadlines.


## Notes

**2026-05-17T10:20:56Z**

Scope update after discovering existing ticket wra-yl7i: this ticket should focus on async payload session implementation and reusable client/event plumbing. The full TUI-to-backend control-socket frontend belongs to wra-yl7i. Do not duplicate that work here; make the payload/TUI bridge compatible with the control client API wra-yl7i defines.
