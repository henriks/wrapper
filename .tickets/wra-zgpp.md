---
id: wra-zgpp
status: open
deps: []
links: []
created: 2026-05-11T20:43:19Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-jyn1
tags: [spike, virtiofs, rust, docs]
---
# Spike and document virtiofsd crate embedding feasibility

Determine whether a repo-local Rust backend can reuse upstream virtiofsd public interfaces at the protocol boundary. The expected target is using VhostUserFsBackend with a repo-owned FileSystem implementation, but the ticket must validate that against real crate APIs.

## Design

Build the smallest possible proof of concept that serves a trivial synthetic filesystem over vhost-user, or document why the public seam is not viable. Record crate version, feature flags, public API names, packaging strategy, and fallback options.

## Acceptance Criteria

The API decision is documented with evidence; either a minimal buildable proof of concept exists or a clear blocker/fallback is documented; later backend tickets have notes if their assumptions changed.

