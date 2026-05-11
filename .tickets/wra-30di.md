---
id: wra-30di
status: open
deps: []
links: []
created: 2026-05-11T20:43:19Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-jyn1
tags: [spike, filesystem, virtiofs, docs]
---
# Spike and document filesystem semantics baseline

Define the minimum filesystem semantics needed for the wrapper's real user experience before implementing ComposedFs. Cover normal agent workflows, Docker bind mounts, auth/state paths, and common developer tooling.

## Design

Document required behavior for lookup, forget, open, release, flush, fsync, getattr, setattr, access, readlink, symlink, mkdir, create, unlink, rename, readdir, xattrs, readonly failures, inode identity, hardlinks, and open-then-rename/unlink behavior. Include smoke commands for git, package manager cache access, Docker bind mounts, Codex/Copilot auth/state access, and shell navigation.

## Acceptance Criteria

A v1 operation/semantics baseline is documented; deliberate semantic compromises are explicit; backend test tickets are updated with the resulting required coverage.

