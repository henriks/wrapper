---
id: wra-udix
status: open
deps: [wra-vy20]
links: []
created: 2026-05-11T20:43:59Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-umuv
tags: [tests, virtiofs, filesystem, security]
---
# Add ComposedFs correctness and adversarial test coverage

Add tests for the composed filesystem core and v1 operation surface, including adversarial path traversal and readonly-boundary behavior. These tests are required because the backend handles untrusted guest filesystem requests.

## Design

Cover synthetic lookup/readdir, mounted subtree delegation, file mounts, nested mount boundaries, ro inside rw, symlink and .. escape attempts, open-then-rename/unlink behavior, hardlink behavior or documented unsupported semantics, concurrent operations, lookup count/inode lifetime, and xattr behavior required by the baseline.

## Acceptance Criteria

Tests can be run by a documented command; security-sensitive cases are covered; any known gaps are documented in ticket notes and linked to follow-up tickets before closing.

