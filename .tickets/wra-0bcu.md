---
id: wra-0bcu
status: closed
deps: [wra-a9je, wra-vy20, wra-udix]
links: [wra-d8nv]
created: 2026-05-11T20:43:59Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-h5hv
tags: [q35, virtiofs, integration]
---
# Integrate q35 composed filesystem host and guest path behind fallback

Integrate the composed backend into the current q35 QEMU launch path and update guest init to consume the composed export in the same implementation slice. This must be behind an explicit non-default switch and must preserve the old per-share virtiofsd path as fallback.

## Design

Generate both composed manifests together: `.sandbox/docker-vm/run/composed-fs-manifest.json` for the host backend and `.sandbox/docker-vm/run/guest-config/composed-binds.json` for guest init. Start `agentvm-composed-fs`, attach its socket/tag to q35, mount the composed export in guest init, and bind selected paths into place from the guest bind manifest. Keep the current tiny config share in v1. Keep networking unchanged. Do not remove `DockerVmGuestShare` or per-share virtiofsd fallback code in this ticket.

This ticket intentionally replaces the earlier separate host-only and guest-only split. Do not add a host-only composed q35 mode that the guest cannot consume; that creates duplicate integration work and proves too little.

## Acceptance Criteria

q35 can boot with the composed backend through an explicit switch; guest init mounts the composed export and binds workspace, tool state, auth/config, system ro paths, and user `--ro`/`--rw` paths; the old per-share path still works; logs and state files are documented; startup failures fail before QEMU when manifest/backend validation fails.


## Notes

**2026-05-12T20:57:18Z**

Dependency insight from wra-a9je: host integration should generate .sandbox/docker-vm/run/composed-fs-manifest.json for the backend and .sandbox/docker-vm/run/guest-config/composed-binds.json for guest init. Keep existing config share in v1. Validate schema, source existence, host access, protected paths, duplicate/overlap conflicts, and kind matches before starting backend or QEMU. See docker/composed-fs-manifest.md.

**2026-05-12T21:08:42Z**

Scaffold handoff from wra-saox: runtime integration should launch composed-fs/target/debug/agentvm-composed-fs in development, or the packaged agentvm-composed-fs binary once release packaging exists. The CLI already supports --manifest .sandbox/docker-vm/run/composed-fs-manifest.json --socket-path .sandbox/docker-vm/run/virtiofs.sock --tag agentvm; QEMU should connect vhost-user-fs-device to that socket/tag. Sandbox-local smoke testing showed Unix listener creation can require host privileges outside the coding sandbox.

**2026-05-12T21:38:17Z**

Started orientation. q35 Docker VM launch is implemented in sandbox-wrap (codex-wrap and copilot-wrap are symlinks to it). Primary project virtiofsd starts in DockerVmManager.start_virtiofsd() using paths.virtiofs_sock and QEMU attaches it as charfs/tag vm_cfg['virtiofs_tag'] in build_qemu_command(). Supplemental guest shares are built in build_guest_shares(), written to guest-config/shares.txt, started by start_guest_shares(), and attached as additional vhost-user-fs-pci devices. Guest config is still a separate readonly virtiofsd on agentvm-config. Integration should add an explicit non-default switch before changing these paths, and must account for the current guest init still expecting per-share tags until wra-d8nv updates it to consume composed-binds.json.

**2026-05-13T05:00:46Z**

Planning correction: do not implement a host-only composed q35 intermediary. Fold the guest-init bind-manifest work from wra-d8nv into this ticket so q35 composed-fs is implemented as one usable end-to-end path. The explicit switch should select either the old per-share stack or the new composed host+guest stack, not a mixed half-state.

**2026-05-13T05:12:51Z**

Implemented integrated q35 composed-fs source changes. sandbox-wrap now has explicit --docker-composed-fs switch, resolves agentvm-composed-fs via SANDBOX_WRAP_COMPOSED_FS_BIN/PATH/composed-fs/target/debug, generates composed-fs-manifest.json and guest-config/composed-binds.json from one mount table, starts the composed backend on the existing primary virtiofs socket, skips per-share QEMU devices only in composed mode, and leaves the default per-share path unchanged. docker/guest-init.sh now detects composed-binds.json, mounts the composed export, and bind-mounts entries; absent composed-binds.json preserves the old project mount plus shares.txt flow. Documented behavior in docker/composed-fs-q35.md. Verification run: python3 -m py_compile sandbox-wrap, sh -n docker/guest-init.sh, cargo build/test --manifest-path composed-fs/Cargo.toml --offline, and a manifest-generation probe. Full appliance rebuild and q35 boot validation remain for wra-oz9h.
