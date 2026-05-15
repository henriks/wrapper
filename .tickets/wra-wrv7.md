---
id: wra-wrv7
status: closed
deps: []
links: [wra-u05c, wra-brsl]
created: 2026-05-15T18:23:42Z
type: bug
priority: 0
assignee: Henrik Saksela
tags: [filesystem, vm, codex, state]
---
# Investigate host tmp deletion through guest writable mounts

A host Codex session lost its sandbox helper under /home/hsaksela/.codex/tmp/arg0 while agentvm/codex-in-vm work was happening. The observed failure in the host session was sandboxed exec/apply_patch failing because /home/hsaksela/.codex/tmp/arg0/codex-*/codex-linux-sandbox no longer existed; escalated commands could still see the checkout. Host /home/hsaksela/.codex/tmp/arg0 was empty from both Codex and user shell views. mtime was around 2026-05-15 19:24:32 Europe/Helsinki; host logs first showed sandboxed exec failure at 2026-05-15T16:27:32Z. A VM run in /home/hsaksela/Code/planb occurred around 19:24:46, but its recorded manifest only showed project-local persistent home, workspace, and host ~/.docker, not host ~/.codex.

The question to answer: why do multiple Codex instances on the same host usually not overlap destructively, but guest/VM-backed sessions may? Specifically check whether exposing host runtime directories through composed-fs/virtiofs changes cleanup, locking, tmp path ownership, or namespace assumptions enough to let a guest remove host tmp helper directories.

Relevant context: prior ticket wra-brsl was closed as superseded because we should not add Codex-specific sharing policy. The project constraint from AGENTS.md is that compatibility matters only for config files, not CLI flags, and code should avoid excessive defensive special cases. User clarified that for this model ~/.codex needs to be mounted writable, but any mitigation must be expressed as generic host-directory mount configuration, not Codex special sauce.

## Design

Start from evidence, not a Codex-specific policy decision. Inspect recent runtime manifests and logs for runs that mounted host ~/.codex or any parent containing ~/.codex/tmp. Confirm whether guest cleanup code, startup scripts, or Codex itself removes tmp/arg0-like directories. Compare same-host Codex behavior against host+guest behavior: file locking through composed-fs, tmp directory naming, and cleanup scope. Document whether the root cause is mount policy, lock propagation, runtime cleanup semantics, or a separate host Codex tmp lifecycle issue.

## Acceptance Criteria

Findings are recorded in this ticket with concrete timestamps, involved paths, and the manifest/log evidence. The conclusion explicitly explains why same-host Codex instances do or do not overlap, and why the VM case can differ. Any implementation follow-up is linked and framed generically in terms of host directory mount behavior rather than Codex-specific filtering. Do not close until the evidence is sufficient for somebody else to reproduce or dismiss the failure mode.


## Notes

**2026-05-15T18:30:33Z**

Preliminary hypothesis review: current runtime_manifest.rs still maps Codex tool state as host_home/.codex -> guest_home/.codex when tool_state.codex is enabled, but the preserved /home/hsaksela/Code/planb/.sandbox/docker-vm/run/composed-fs-manifest.json from the observed 2026-05-15 19:24 run contains only persistent-home, workspace, and host ~/.docker mounts; it does not expose host ~/.codex. /home/hsaksela/Code/planb/.sandbox/home also has no .codex symlink/dir. That specific manifest therefore could not directly remove host ~/.codex/tmp/arg0 unless there was another run/manifest or a different path exposure not yet found. Plausible mechanism if ~/.codex is mounted writable in another run: Codex tmp helper cleanup may rely on host-local namespace assumptions such as PID/proc visibility, lock files, or temp ownership; a guest Codex process seeing host ~/.codex through virtiofs may not see host processes/locks the same way and could classify host arg0 helper dirs as stale and rm them. Need reproduce with explicit writable ~/.codex mount plus strace/inotify/audit around ~/.codex/tmp/arg0.

**2026-05-15T18:38:01Z**

Closed per user direction after agreeing that an application cleanup mechanism is the likely explanation and that locks are not the suspected missing primitive. The concrete mitigation work moved to generic share shadows in wra-u05c so volatile child paths such as tmp/arg0 can be backed by project-local state while the parent writable host share remains mounted. wra-u05c is implemented and validated.
