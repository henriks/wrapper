---
id: wra-6uak
status: closed
deps: []
links: []
created: 2026-05-18T05:36:48Z
type: chore
priority: 2
assignee: Henrik Saksela
parent: wra-9m5h
tags: [cleanup, repo-hygiene]
---
# Delete tracked legacy and iteration artifacts

The repository still tracks obsolete iteration artifacts: old/ contains legacy wrapper scripts and notes, .ralph/ contains stale task-state markdown/json files, and .codex is tracked as a process artifact. These files are not production source, validation, or project documentation and should be removed from the tracked tree once confirmed unused.

## Design

Review references to old/, .ralph/, and .codex. Delete the tracked artifacts that are not part of the current product or validation flow. Add or adjust ignore rules only for artifacts that tools may recreate. Do not remove active ticket data under .tickets.

## Acceptance Criteria

old/, .ralph/, and the tracked .codex artifact are removed or each retained with a documented product purpose; ignore rules prevent accidental reintroduction of generated process files; no code or validation flow depends on the deleted artifacts.


## Notes

**2026-05-18T06:40:08Z**

Removed tracked repo/process artifacts: old/, tracked stale .ralph loop files, and the empty project-root .codex artifact. Added root-scoped ignores for /.ralph/ and /.codex so Ralph/Codex state can be recreated locally without being tracked. Verified no non-ticket/non-artifact references to old/ or .ralph/ remain; .codex references are product tool-state paths, not the deleted project-root artifact. Ran ./vm-frontend/validate.sh required successfully including live-smoke and live-setup-tools.
