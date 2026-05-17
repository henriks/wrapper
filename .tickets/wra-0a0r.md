---
id: wra-0a0r
status: closed
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

**2026-05-17T20:40:50Z**

Starting after vmnet poller replacement closed. First slice will be mechanical module-boundary cleanup only: extract the vhost-user server/CLI boundary from composed-fs/src/lib.rs while preserving synchronous bounded-blocking filesystem request execution and existing ServeConfig/run_cli/serve_vhost_user_fs public API. No Tokio dependency or config semantics changes.

**2026-05-17T20:41:55Z**

First mechanical module-boundary slice: extracted the composed-fs vhost-user server/CLI boundary from composed-fs/src/lib.rs into composed-fs/src/server.rs. Public API is preserved via re-export of ServeConfig, run_cli, and serve_vhost_user_fs; filesystem request execution remains synchronous and uses the existing vhost-user thread pool size. No Tokio dependency/config semantics changes. Focused validation passed: cargo fmt --manifest-path composed-fs/Cargo.toml; cargo test --manifest-path composed-fs/Cargo.toml --offline -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline exposes_vmnet_runtime_config_for_supervisor -- --nocapture.

**2026-05-17T20:44:06Z**

Reflection checkpoint: progress on wra-0a0r is intentionally mechanical so far: server/CLI boundary extracted, filesystem request execution still synchronous, and the vhost thread-pool size remains the explicit bounded-blocking control. Added ServeConfig::validate so zero worker count fails fast as InvalidInput and tests document the default one-worker bounded-blocking behavior. Focused validation passed: cargo fmt --manifest-path composed-fs/Cargo.toml; cargo test --manifest-path composed-fs/Cargo.toml --offline serve_config -- --nocapture; cargo test --manifest-path composed-fs/Cargo.toml --offline --lib -- --nocapture.

**2026-05-17T20:45:36Z**

Second module-boundary slice: extracted manifest schema types and validate_manifest_json_shape from composed-fs/src/lib.rs into composed-fs/src/manifest.rs. The root module re-exports validate_manifest_json_shape and keeps crate-private schema types available to Namespace/tests. No manifest semantics changed; existing manifest validation and proptest/fuzz harness entry points are preserved. Focused validation passed: cargo fmt --manifest-path composed-fs/Cargo.toml; cargo test --manifest-path composed-fs/Cargo.toml --offline --lib -- --nocapture.

**2026-05-17T20:49:37Z**

Third composed-fs slice: extracted handle/lock table state from composed-fs/src/lib.rs into new composed-fs/src/state.rs (FileHandle, HandleTable, LockKey, LockTable), keeping it crate-private and synchronous. Also added ComposedFs lock-access helpers that convert poisoned namespace/handle/lock table locks into io::Error instead of panicking in result-returning paths, and narrowed release() so the handle table write lock is dropped before optional sync_all() on flush. Existing forget/batch_forget lack an error return, so they now no-op on poisoned namespace lock instead of panicking. Focused validation passed: cargo fmt --manifest-path composed-fs/Cargo.toml; cargo test --manifest-path composed-fs/Cargo.toml --offline --lib -- --nocapture.

**2026-05-17T20:53:17Z**

Added focused poison-path coverage for the new composed-fs lock-access helpers: namespace, handle table, and lock table poisoning now return io::Error in exercised paths instead of panicking. Also kept focused validation green after the state split and release lock-scope cleanup: cargo fmt --manifest-path composed-fs/Cargo.toml; cargo test --manifest-path composed-fs/Cargo.toml --offline poisoned_state_locks -- --nocapture; cargo test --manifest-path composed-fs/Cargo.toml --offline --lib -- --nocapture.

**2026-05-17T20:56:37Z**

Required/live-capable validation passed after composed-fs bounded-blocking refactor slices: ./vm-frontend/validate.sh required. This validates formatting, composed-fs/guest-service/payload-protocol/vm-frontend offline tests, fuzz target compilation, docs drift checks, live-smoke, and live-setup-tools with the current Python-default appliance. Acceptance for this option-1 composed-fs slice is met: server/manifest/state module boundaries extracted; vhost thread_pool_size is explicitly positive/bounded; no Tokio dependency or async filesystem execution was introduced; lock poison paths are recoverable in result-returning code; release() avoids holding the handle table lock across flush sync_all(); existing property/fuzz harness coverage is preserved.
