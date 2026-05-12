---
id: wra-jyn1
status: open
deps: []
links: []
created: 2026-05-11T20:43:04Z
type: epic
priority: 1
assignee: Henrik Saksela
parent: wra-myui
tags: [qemu, microvm, virtiofs, spike, docs]
---
# Document feasibility for microvm and composed virtio-fs

Discovery epic for the required spikes before implementation. Each spike is successful only when it documents tested commands/configuration, observed behavior, constraints, decisions, rejected alternatives, and follow-up ticket changes. These tickets are meant to prevent duplicate investigation work.

## Design

Run small targeted experiments only. Do not implement the production composed filesystem here except for minimal proof-of-concept code needed to prove API feasibility.

## Acceptance Criteria

All feasibility spike tickets are closed and their outcomes are documented in plan.md, an adjacent docker design note, or ticket notes.


## Notes

**2026-05-12T20:49:03Z**

Progress update: closed wra-mjer and wra-zgpp. Microvm/device feasibility is documented in docker/microvm-spike.md; virtiofsd embedding feasibility is documented in docker/virtiofsd-embedding-spike.md. Remaining child for this discovery epic is wra-30di, the filesystem semantics baseline.
