---
id: wra-3z80
status: closed
deps: [wra-fqw7]
links: []
created: 2026-05-15T19:47:11Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-ia1a
tags: [docker, vm, filesystem, persistence]
---
# Add separate Docker disk only if root-overlay Docker fails

Contingency implementation ticket: if wra-fqw7 proves Docker cannot reliably use /var/lib/docker on the persistent root overlay, add a second project-local sparse ext4 disk mounted at /var/lib/docker. This should be treated as a technical workaround, not the primary persistence model. If Docker works on the root overlay, close this ticket as unnecessary with validation evidence.

Context: current code already has a Docker-only data disk, but the desired architecture is root overlay persistence. Do not keep the old docker-data.raw path/name/semantics just for compatibility; if needed, implement a clearly named optional Docker state disk with docs explaining why it exists.

## Design

Base the decision on wra-fqw7 artifacts. If needed, create/format a second sparse disk, attach it separately, mount it at /var/lib/docker after overlay root is active and before dockerd starts, and validate Docker persistence across relaunch. If not needed, remove the Docker-only disk entirely as part of cleanup.

## Acceptance Criteria

Either separate Docker disk support exists only with documented validation justification and tests, or this ticket is closed as not needed because Docker-on-root-overlay validation passed.


## Notes

**2026-05-15T20:09:55Z**

Closed as unnecessary based on wra-fqw7 validation. Docker works on the persistent root overlay in live validation after ensuring guest writes are synced before the frontend terminates QEMU. No separate Docker disk should be added unless future validation uncovers a concrete Docker-on-overlay failure.
