---
id: wra-t79v
status: open
deps: [wra-9xru]
links: []
created: 2026-05-11T20:44:16Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-3o6e
tags: [cleanup, virtiofs, rollout]
---
# Remove old per-share virtiofsd export path after fallback window

Remove the old per-share virtiofsd/socket/device path only after composed fs and microvm have passed validation and the fallback window is complete. If the old path must remain, document the reason instead of leaving it accidentally.

## Design

Delete or simplify DockerVmGuestShare process management, per-share sockets, per-share guest mounting logic, and related docs once it is safe. Preserve runtime-contract documentation accuracy.

## Acceptance Criteria

Old per-share code is removed or explicitly retained with rationale; docs no longer describe obsolete default behavior; cleanup does not remove needed fallback before rollout criteria are met.

