---
id: wra-0a0r
status: open
deps: [wra-9glk]
links: []
created: 2026-05-17T10:19:57Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, composed-fs, refactor]
---
# Option 1: refactor composed-fs as bounded blocking vhost/filesystem backend

Refactor composed-fs as idiomatic synchronous Rust with bounded blocking vhost/filesystem request execution, rather than forcing Tokio into vhost-user and host filesystem callback paths. composed-fs/src/lib.rs is large and uses shared namespace, handle, and lock state behind RwLock/Mutex.

## Design

Split lib.rs into modules such as manifest, namespace, path resolution, handle table, lock table, FUSE ops, syscall wrappers, server, and tests/fuzz harness. Audit lock scopes to avoid holding locks across host filesystem calls where possible. Replace runtime lock poison expect paths with recoverable io::Error where appropriate. Profile and tune the existing vhost worker/thread-pool model before considering any deeper async transport rewrite. If a future vhost transport/control plane becomes async, keep filesystem request execution behind bounded blocking workers unless measurements prove this cannot meet stability and performance goals. Preserve fuzz/property coverage for arbitrary filesystem input.

## Acceptance Criteria

composed-fs remains synchronous at the filesystem request execution layer, module boundaries are clear, lock scopes are smaller and documented by tests where risky, arbitrary input fuzz coverage is preserved or expanded, worker-pool sizing/contention has been profiled or made explicit, and no Tokio runtime dependency is introduced solely for composed-fs.


## Notes

**2026-05-17T10:28:44Z**

Async-boundary refinement: this ticket is not just "do not add Tokio to composed-fs"; it should actively improve the synchronous backend. Prioritize module split, lock-scope audit, avoiding namespace/handle/lock guards across slow host filesystem work where possible, recoverable lock-poison errors, and profiling/tuning of the existing vhost thread pool. Full async vhost/filesystem rewrite is out of scope unless backed by measurements showing lock/thread-pool tuning cannot meet stability/performance goals.
