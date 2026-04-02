---
id: wra-snm9
status: open
deps: [wra-mkh8, wra-eh7c]
links: []
created: 2026-04-01T21:17:14Z
type: feature
priority: 0
assignee: Henrik Saksela
parent: wra-oszw
tags: [vm, cleanup, launcher]
---
# Remove Bubblewrap and collapse sandbox-wrap to a VM-only launcher

Delete the hybrid execution path and make `sandbox-wrap` a focused VM launcher/supervisor. Today `sandbox-wrap` still contains both a substantial QEMU supervisor and a large Bubblewrap command builder. Once guest execution and guest state sharing exist, the old host-side sandbox should be removed instead of left behind.

Scope:
- Remove `build_bwrap_args()` and the Bubblewrap-specific mount/environment path.
- Remove the `bwrap` prerequisite and all code that exists only to launch the agent on the host.
- Simplify `main()` so the wrapper has one primary execution path: prepare runtime, boot VM, run payload in guest, supervise lifecycle, tear down.
- Revisit the hidden proxy/helper structure and keep only what remains necessary after the VM-only rewrite.
- Restructure the file as needed so the final code is materially tighter and easier to reason about than the current monolith.

Relevant code:
- `sandbox-wrap` broadly, especially `build_bwrap_args()`, `parse_args()`, and `main()`.

This ticket is the deletion-heavy pivot that should produce the code-size and structure win requested by the user.

## Design

Bias toward fewer branches and fewer execution modes. If a piece of logic only existed to support the old Bubblewrap path, delete it. Favor small helper functions or modules over preserving one huge script with dead conceptual layers removed only in spirit.

## Acceptance Criteria

The agent path no longer depends on Bubblewrap.

The wrapper has one primary VM-only execution flow.

The resulting code is materially smaller or at least structurally simpler than the current hybrid implementation.

## Notes

**2026-04-01T21:25:34Z**

Contract decisions from wra-fj7n: --docker is removed because the VM is now the default execution model, and the old host-side Bubblewrap path should be deleted rather than retained behind compatibility branches. The cleanup target is one primary flow only: host wrapper prepares state, boots the VM, runs the payload in the guest, supervises lifecycle, and tears down.

**2026-04-01T21:27:09Z**

User requirement update from after wra-fj7n: deleting Bubblewrap still stands, but removal of the old host-side path matrix does not mean removing arbitrary path mounts entirely. The cleanup ticket should delete Bubblewrap-specific machinery while keeping a smaller VM-era implementation of --ro/--rw guest shares.

**2026-04-02T07:06:26Z**

Implementation insight from wra-mkh8: once the VM-only path is made default, the cleanup ticket should delete the old Bubblewrap bind matrix but keep the new smaller guest-share layer (fixed config share + supplemental virtio-fs shares) as the supported implementation of --ro/--rw and tool/auth mounts.
