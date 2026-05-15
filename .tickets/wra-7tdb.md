---
id: wra-7tdb
status: closed
deps: []
links: []
created: 2026-05-15T16:38:38Z
type: task
priority: 1
assignee: Henrik Saksela
---
# Remove wrapper compatibility CLI flags

Remove the user-facing wrapper compatibility flags --tool, --tool-arg, and --command from agentvm/agentvm-frontend wrap. The intended UX is: durable setup via agentvm --setup-tool codex|pi, one-run command override via agentvm -- COMMAND [ARG...], and config.json for durable defaults. Update vm-frontend/src/main.rs parser/tests plus requirements.md and docker/runtime-contract.md so these flags are no longer documented as compatibility behavior. Also audit tickets/docs/code for other compatibility paths still present and report them.

## Acceptance Criteria

agentvm wrapper rejects or no longer accepts --tool, --tool-arg, and --command; post--- command override continues to work; --setup-tool codex|pi continues to work; docs no longer present those flags as wrapper compatibility; tests cover removal; validation gate result is documented.


## Notes

**2026-05-15T18:10:15Z**

Implemented removal of the wrapper/CLI compatibility flags --tool, --tool-arg, and --command. vm-frontend now launches configured setup recipes through explicit payload scripts plus --tool-state, so Codex no longer depends on the old launch-time GuestTool path. Removed GuestTool/Copilot compatibility plumbing from runtime manifests and low-level launch/self-test parsing. Updated requirements, runtime contract, UX/TUI docs, validation docs, and tests. Validation: ./vm-frontend/validate.sh required passed, including live-smoke.
