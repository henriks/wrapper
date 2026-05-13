---
id: wra-nrz2
status: closed
deps: []
links: []
created: 2026-05-13T10:21:55Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, virtiofs, filesystem]
---
# Refactor composed filesystem for embedding in Rust frontend

Prepare the existing composed-fs Rust implementation so the Rust frontend can embed or directly reuse it instead of supervising a redundant external filesystem helper from Python. Current composed-fs is validated as the backend for microvm/q35 composed virtio-fs. The pivot should preserve that implementation and expose a clean Rust API for manifest loading, vhost-user socket serving, logging, lifecycle, and error reporting.

## Design

Split reusable composed filesystem logic from binary-only CLI concerns where needed. Keep the existing agentvm-composed-fs binary working during migration, but make the frontend able to start the backend in-process or through a well-defined Rust-owned task. Document the chosen boundary. Do not fork a second filesystem implementation. Preserve current manifest semantics, readonly enforcement, operation tests, and daemon.wait lifecycle fix.

## Acceptance Criteria

The frontend has a documented Rust integration path for composed fs. Existing composed-fs tests still pass. There is no duplicate filesystem implementation. Any retained external binary mode is explicitly a bridge for migration, not the target architecture.


## Notes

**2026-05-13T10:35:07Z**

Refactor started by moving composed-fs implementation from a binary-only main.rs into src/lib.rs, adding ServeConfig and serve_vhost_user_fs(), and replacing src/main.rs with a thin CLI wrapper. This keeps one filesystem implementation while making the Rust frontend able to run the vhost-user filesystem in its own process/thread ownership model.

**2026-05-13T10:36:04Z**

Embedding path documented in composed-fs/README.md. The public API is agentvm_composed_fs::{ServeConfig, serve_vhost_user_fs, run_cli}; serve_vhost_user_fs is blocking and should be run by the frontend in a dedicated thread/blocking task. Existing agentvm-composed-fs binary remains a thin migration bridge over the same library, not a duplicate implementation. Validation: cargo test --manifest-path composed-fs/Cargo.toml --offline passed, including 17 library tests and the thin binary test target.
