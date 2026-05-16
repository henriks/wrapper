---
id: wra-t9s7
status: open
deps: []
links: [wra-ylt9]
created: 2026-05-16T15:50:48Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [docs, validation, sqlite]
---
# Align validation docs with skipped SQLite concurrency scope

Problem:
Required live-smoke currently skips host/guest SQLite concurrency while wra-ylt9 remains open, but some validation/operations docs still imply required validation exercises that concurrency path.

Relevant code/docs:
- vm-frontend/validate.sh:158-165 live-smoke uses --skip-sqlite-concurrency.
- vm-frontend/validation-workflow.md:179 and 187 describe validation scope.
- docker/OPERATIONS.md:129 mentions sqlite-concurrency-smoke.
- Existing open ticket: wra-ylt9 tracks the underlying SQLite concurrency investigation.

Impact:
Developers can make incorrect closure decisions or misread required validation coverage.

Recommended fix:
Update docs to state concurrency is intentionally skipped pending wra-ylt9, document the opt-in command separately, and extend docs drift checks if practical.

Validation:
- Run documentation drift checks and required validation docs tests.
- Link this ticket to wra-ylt9.

