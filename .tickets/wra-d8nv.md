---
id: wra-d8nv
status: open
deps: [wra-a9je]
links: []
created: 2026-05-11T20:43:59Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-h5hv
tags: [guest-init, virtiofs, integration]
---
# Update guest init to bind from composed export

Replace share-per-tag guest reconstruction with mounting the composed export at a stable internal path and bind-mounting selected manifest paths into final guest absolute locations.

## Design

Use the guest bind manifest from the schema ticket. Define parent directory creation, required versus optional binds, failure behavior, and how boot config is delivered. Do not remove the current config share unless a replacement is implemented and documented.

## Acceptance Criteria

Guest init can mount the composed export and bind workspace, tool state, auth/config, system ro paths, and user --ro/--rw paths; failure behavior is clear in logs; the old share-per-tag logic remains available until fallback removal.

