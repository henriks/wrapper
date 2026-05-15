---
id: wra-sne0
status: closed
deps: []
links: []
created: 2026-05-15T08:55:11Z
type: epic
priority: 2
assignee: Henrik Saksela
---
# Replace verbose custom code with focused external crates

Track targeted replacements of verbose hand-written code with focused Rust crates where the crate improves correctness, maintainability, or UX without weakening sandbox semantics. Scope includes composed-fs syscall wrappers, packet parsing/building, temp dirs, shell quoting, project locks, process timeouts, CLI parsing cleanup, TUI input helpers, and small error/display helpers. See child tickets for per-area context, candidate crates, and risk notes.

## Acceptance Criteria

Each accepted replacement has a dedicated ticket with context, candidate crate, risk/tradeoff, validation command, and dependency ordering. High-risk security-boundary migrations are split into spike/design and incremental implementation tickets rather than broad rewrites.


## Notes

**2026-05-15T08:57:15Z**

Created child tickets from the crate-replacement review. Recommended low-risk first batch: tempfile for test dirs (wra-a9nb), wait-timeout for process waits (wra-clxy), fs4 for project locks (wra-eqmj), shlex for shell quoting (wra-4rmu), and clap for composed-fs CLI (wra-i4qr). Higher-risk or larger design work is split: rustix starts with a spike (wra-easf) before implementation (wra-mw81); readiness-driven vmnet runtime starts as a mio spike (wra-0s2s). Packet parsing is tracked separately in wra-jtgg, with a preference to use existing smoltcp::wire before adding etherparse.

**2026-05-15T09:20:30Z**

Implemented and closed all child tickets. External crates now in use where accepted: tempfile, wait-timeout, fs4, shlex, clap in composed-fs, thiserror, smoltcp packet APIs, pcap-file, tui-input, unicode-width, and rustix for the first open/path syscall slice. The mio runtime rewrite was evaluated and intentionally deferred as a separate design effort. Validation completed with vm-frontend and composed-fs offline tests plus fmt checks.
