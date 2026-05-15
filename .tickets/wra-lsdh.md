---
id: wra-lsdh
status: closed
deps: []
links: []
created: 2026-05-15T06:42:46Z
type: task
priority: 0
assignee: Henrik Saksela
tags: [fs, locks, tests, proptest]
---
# Add property fuzz coverage for composed-fs byte-range locks

Extend the existing composed-fs property/fuzz test sets to cover POSIX byte-range lock behavior added by wra-zfib. Deterministic unit tests cover representative cases, but exhaustive confidence needs generated owner/range/read-write/unlock interleavings checked against a model. Scope: composed-fs/src/lib.rs tests around getlk/setlk/setlkw, owner isolation, shared read locks, write exclusion, subrange unlocks, flush/release cleanup, readonly handle behavior, and host lock conflict assumptions where feasible.

## Design

Add proptest-generated lock operation sequences using small byte ranges and several guest owners. Keep the model independent from the implementation: model active locks as owner/type/range intervals with POSIX-compatible merge/split behavior, shared read compatibility, write exclusion, and unlock subrange removal. Execute equivalent operations through ComposedFs and compare success/failure and GETLK conflict/no-conflict outcomes. Include a stress/ignored variant for longer sequences if the normal fast case must stay small.

## Acceptance Criteria

Fast cargo test includes at least one generated lock operation property test; an ignored longer stress test exists for lock sequences; tests fail against the old no-lock/EOPNOTSUPP implementation and pass against the new bridge; ticket notes record verification commands and any model limits.


## Notes

**2026-05-15T06:45:27Z**

Added property/fuzz coverage for composed-fs byte-range locks in composed-fs/src/lib.rs. The fast test proptest_lock_operation_sequences_cover_owner_and_range_interleavings generates SETLK, GETLK, and flush(owner) operations across 4 guest owners and small byte ranges, comparing ComposedFs behavior to an independent interval-lock model with shared read compatibility, write exclusion, owner-scoped subrange unlock, and owner flush cleanup. Added ignored long stress test proptest_lock_operation_sequences_stress with 512 generated cases and longer sequences. During development the property test caught an overly strict GETLK model assumption: Linux may return any conflicting lock, not the first model lock; the assertion now validates that the returned lock matches some valid model conflict. Verification passed: focused fast property, ignored stress property, full cargo test --manifest-path composed-fs/Cargo.toml --offline, full cargo test --manifest-path vm-frontend/Cargo.toml --offline, git diff --check, and tk dep cycle.
