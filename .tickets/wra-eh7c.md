---
id: wra-eh7c
status: open
deps: [wra-fj7n]
links: []
created: 2026-04-01T21:17:14Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-oszw
tags: [vm, guest, auth, workspace]
---
# Move tool state, auth, and workspace sharing fully into the guest

Replace the current host-side Bubblewrap bind strategy with a smaller, explicit set of host shares consumed directly by the guest. The current wrapper has a large amount of path-specific mount logic for `HOME`, `~/.codex`, `~/.docker`, `~/.config/gh`, `~/bin` symlink targets, extra `ro`/`rw` binds, and environment shaping. In the VM-only model, those concerns should move to a tighter guest-share design.

Scope:
- Decide and implement which host paths are made available to the guest for each supported tool.
- Preserve project workspace access through `virtio-fs` at the original project path and `/workspace` alias if still useful.
- Make Codex/Copilot state directories available in a deliberate way rather than by replaying the full Bubblewrap `HOME` layout.
- Preserve Docker client config, GitHub auth, and optional AWS credential behavior only where still needed.
- Re-evaluate whether `--ro`, `--rw`, and `--pass-env` survive, and if they do, map them to the new VM-only model.

Relevant code:
- `sandbox-wrap`: `TOOLS`, `ensure_sandbox_mise_config()`, `build_bwrap_args()` mount and env logic.
- `docker/guest-init.sh` and any new guest-side share-mount logic.

The target is to preserve necessary functionality while materially shrinking the number of special cases.

## Design

Do not recreate a full host HOME inside the guest unless a concrete requirement demands it. Prefer a small number of explicit shares and simple guest-side mount conventions. The point of this ticket is to simplify the model, not to clone the host session inside the VM.

## Acceptance Criteria

The selected tool has the project workspace and the minimum required persistent state inside the guest.

Auth/config flows that remain supported still work under the VM-only model.

The implementation is visibly smaller and simpler than the current host-side bind/env matrix.

## Notes

**2026-04-01T21:25:30Z**

Contract decisions from wra-fj7n: persistent guest HOME is .sandbox/home/ rather than .sandbox/ directly, and the guest should consume a deliberately small set of shares instead of replaying the old Bubblewrap HOME layout. Removed flags are --ro, --rw, and --pass-env, so this ticket should not preserve a generic host path/env injection matrix unless a concrete VM-era requirement emerges.

**2026-04-01T21:27:09Z**

User requirement update: arbitrary host path mounts are needed in the VM-only design. This ticket should preserve --ro and --rw as explicit guest-share features at the same absolute path, while still avoiding a full recreation of the old Bubblewrap HOME/session model. --pass-env is still out.

**2026-04-02T05:31:58Z**

Dependency insight from wra-mkh8: the guest payload path now expects a persistent guest HOME at <project>/.sandbox/home and will be much more useful once tool/auth state is shared there. The execution/control ticket intentionally did not complete host auth/config sharing; Codex/Copilot usability in the guest still depends on this follow-up ticket.
