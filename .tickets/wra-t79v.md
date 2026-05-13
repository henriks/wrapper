---
id: wra-t79v
status: closed
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


## Notes

**2026-05-13T06:35:04Z**

Default switch completed in wra-3o6e: --docker now uses microvm composed mode; old q35 per-share path is intentionally retained behind --docker-legacy-per-share-fs for the fallback window. When this ticket runs, remove or re-justify that flag, DockerVmGuestShare process startup for per-share virtiofsd, shares.txt guest reconstruction, and docs that describe the legacy path as available.

**2026-05-13T06:48:37Z**

Removed the redundant legacy per-share virtiofsd path instead of preserving it. Host cleanup: removed --docker-legacy-per-share-fs and --docker-composed-fs flags, removed composed_fs_enabled branching, removed DockerVmGuestShare per-share sockets/pids/logs/procs, removed shares.txt generation, removed start_virtiofsd/start_guest_shares, removed per-share QEMU devices, and kept only composed backend plus config share for both microvm default and explicit q35. Guest cleanup: removed mount_extra_share and shares.txt reconstruction; guest-init now requires composed-binds.json and fails boot if it is missing. Docs updated to state that no per-share fallback remains. Validation: py_compile, sh -n guest-init, docker/check-qemu-command-shape.py, composed-fs cargo test offline, default --docker microvm smoke, explicit --docker-machine q35 smoke, and removed flag rejected by argparse. Note: docker/out initrd must be rebuilt before the guest-init fallback removal is present in appliance artifacts.
