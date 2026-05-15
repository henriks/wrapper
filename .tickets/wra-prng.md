---
id: wra-prng
status: closed
deps: [wra-2kah]
links: []
created: 2026-05-15T06:07:39Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-zfib
tags: [docs, fs, locks, sqlite]
---
# Advertise POSIX_LOCKS and document lock-supported filesystem contract

After implementation and validation, turn on the FUSE POSIX_LOCKS capability for composed-fs and update the runtime contract/docs to state the supported lock semantics. This replaces the current conservative behavior where composed-fs does not advertise POSIX locks and returns EOPNOTSUPP. The docs should make clear that shared writable host/guest mounts are intended to support SQLite-style workloads through generic filesystem semantics, not Codex-specific state policy.

## Design

Update ComposedFs::init capability handling only after lock bridge tests and live SQLite validation pass. Revise docker/filesystem-semantics-baseline.md, docker/runtime-contract.md, requirements.md, validation docs, and any relevant plan notes. Include platform caveats and fallback behavior. Remove or update tests/docs that describe POSIX locks as deferred, while preserving tests that unsupported cases fail explicitly.

## Acceptance Criteria

POSIX_LOCKS is advertised for supported composed-fs mounts; docs describe the lock contract, validation commands, and unsupported-platform behavior; stale docs claiming locks are deferred are removed; all fast tests pass and the live SQLite lock validation is documented with exact command and outcome.


## Notes

**2026-05-15T06:41:51Z**

Rolled out lock-supported contract. ComposedFs::init now advertises POSIX_LOCKS when the guest offers it. Docs updated in docker/filesystem-semantics-baseline.md, docker/composed-fs-operations.md, docker/OPERATIONS.md, vm-frontend/validation-workflow.md, and plan.md to describe POSIX byte-range locks as supported through host OFD locks rather than deferred. Fast verification passed: cargo test --manifest-path composed-fs/Cargo.toml --offline; cargo test --manifest-path vm-frontend/Cargo.toml --offline; python3 -m py_compile docker/guest-payload-server.py; sh -n docker/guest-init.sh; git diff --check; tk dep cycle. Live SQLite validation passed with temporary patched rootfs: guest and host each wrote 200 WAL rows and PRAGMA integrity_check returned ok; full self-test still failed later at Docker TCP timeout, documented on wra-2kah.
