---
id: wra-12nu
status: open
deps: [wra-rysl]
links: []
created: 2026-05-15T06:51:13Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-2cfd
tags: [filesystem, concurrency, fuzzing]
---
# Add deterministic composed-fs concurrency and mutation interleaving tests

Add deterministic adversarial interleaving coverage for composed-fs race classes that are not exercised by the current sequential property model. Relevant code: composed-fs/src/lib.rs tests, safe traversal/open/lookup/forget/readdir/handle paths in the ComposedFs implementation. Cover staged host and guest interleavings around lookup then host symlink swap before open, parent rename/recreate after child lookup, open handle plus unlink/rename/truncate, readdir snapshot while host mutates entries, lookup/forget count churn, concurrent create/rename/unlink against the same directory, and lock interactions where guest owners and host POSIX locks overlap. Prefer deterministic barriers/channels over sleep-based races; keep traces useful for tickets.

## Acceptance Criteria

New tests exercise multiple host/guest interleaving classes without relying on flaky timing. They fail closed on path escape attempts, preserve expected open-handle semantics, and document any intentionally unsupported race behavior. The fast tier includes cheap deterministic cases; heavier variants are wired into the stress tier if needed.

