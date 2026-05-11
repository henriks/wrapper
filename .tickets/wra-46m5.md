---
id: wra-46m5
status: open
deps: [wra-saox, wra-a9je, wra-30di]
links: []
created: 2026-05-11T20:43:42Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-umuv
tags: [virtiofs, filesystem, security]
---
# Implement ComposedFs namespace, inode, and safe traversal core

Implement the core ComposedFs namespace model: synthetic directories, mount roots, delegated host nodes, inode allocation/identity, lookup count tracking, and fd-relative safe host traversal beneath each mount root.

## Design

Use the documented filesystem semantics baseline. Avoid naive host path string concatenation. Enforce mount boundaries and prevent symlink or .. escape. Document the final inode identity policy, including any hardlink or rename compromises.

## Acceptance Criteria

Synthetic lookup/readdir, dir mounts, file mounts, nested boundaries, lookup/forget behavior, and safe traversal are implemented and covered by tests; semantic compromises are documented before closing.

