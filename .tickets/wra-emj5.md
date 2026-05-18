---
id: wra-emj5
status: closed
deps: []
links: [wra-piqm, wra-9m5h]
created: 2026-05-18T10:35:11Z
type: epic
priority: 1
assignee: Henrik Saksela
tags: [cleanup, technical-debt, agentvm, follow-up]
---
# Cleanup follow-up: collapse remaining adapters and compatibility islands

Follow-up cleanup epic after wra-9m5h closed. Three duplicate scans on 2026-05-18 found residual technical debt that survived the first cleanup wave: sync/async payload split, sync CLI/runtime islands, Python guest Docker bridge, non-config guest compatibility aliases, remaining oversized modules, and open wra-piqm tickets that could add adapters instead of replacing old behavior. Stability and performance remain nonfunctional end goals, but the success criterion is minimum necessary code.

## Design

Each child ticket must name code to delete, merge, or make single-owner. Do not reopen wra-9m5h. Do not add compatibility shims except for .sandbox/config.json. Existing wra-piqm tickets should be updated so their fixes replace old behavior instead of layering new wrappers around it. Prefer one async application edge, one payload session architecture, one artifact metadata path, and one guest-service ownership model.

## Acceptance Criteria

All child tickets are implemented or deliberately superseded; related wra-piqm tickets have cleanup-direction notes or dependencies; no newly added cleanup ticket preserves duplicate sync/async paths; dependency cycles are absent; code-changing tickets record ./vm-frontend/validate.sh required or a live-environment limitation before close.


## Notes

**2026-05-18T10:41:08Z**

Ralph loop started in .ralph/wra-emj5.md. Initial setup: epic set in_progress; tk dep cycle reports no cycles. Ready child set includes wra-3s2f, wra-dmmq, wra-6qa2, wra-b9b1, wra-do0x, wra-hizo, wra-g34z, wra-w8ea, and wra-fpy2. Sudo appliance rebuild watchlist captured for appliance-input tasks (notably wra-hizo, wra-b9b1, and possibly wra-do0x).

**2026-05-18T10:58:46Z**

Iteration 5 reflection: completed two high-priority cleanup tickets already (wra-dmmq, wra-6qa2) with required validation. Current active risk is parallel large changes: wra-3s2f is frontend-only and should be finished before appliance-changing wra-b9b1 to keep validation failures attributable. wra-b9b1 remains audited but should not be closed until Henrik performs a sudo appliance rebuild after implementation. Next best step is migrate TUI payload viewport to AsyncPayloadSession/AsyncPayloadCommandSender, then remove or demote sync PayloadSession/Runner/Writer to tests only.

**2026-05-18T11:01:58Z**

Iteration 6 reflection: wra-dmmq and wra-6qa2 are closed with required validation. wra-3s2f is the active focus; plain launch and TUI now use AsyncPayloadSession with PayloadSessionOutcome, but payload-client CLI/diagnostics and sync tests still keep PayloadSession/Runner/Writer alive. wra-b9b1 remains audited but deferred until payload refactor is stable because it will touch appliance inputs and require sudo appliance rebuild/live validation.

**2026-05-18T12:29:58Z**

Iteration 19: wra-w8ea closed after ./vm-frontend/validate.sh required passed. tk dep cycle reports no cycles, but epic close is blocked by open children wra-p06p and wra-39g2. Both remaining children are appliance-input tasks and will require notifying Henrik plus sudo ./docker/build-appliance.sh and live/required validation after implementation.

**2026-05-18T12:35:34Z**

Reflection iteration 21: all original cleanup children are closed; remaining added children wra-p06p and wra-39g2 are implemented with local validation but blocked on sudo appliance rebuild. Current docker/out artifact manifest is expectedly stale for the new source-input freshness contract because it lacks guest-service/payload-protocol source hashes. Do not close remaining children or epic until Henrik rebuilds and ./vm-frontend/validate.sh required passes.

**2026-05-18T12:36:45Z**

Iteration 23 blocker check: docker/out/artifact-manifest.json is still from the pre-wra-39g2 build and has only 5 source_inputs; it is missing Cargo.toml, Cargo.lock, guest-service/Cargo.toml, guest-service/src/lib.rs, guest-service/src/main.rs, payload-protocol/Cargo.toml, and payload-protocol/src/lib.rs. tk ready still lists wra-p06p and wra-39g2 in_progress, and tk dep cycle reports no cycles. Continue waiting for Henrik to rerun sudo ./docker/build-appliance.sh before required/live validation.

**2026-05-18T12:37:39Z**

Iteration 24 blocker check: artifact manifest still has only 5 source_inputs and is still missing the new Cargo/guest-service/payload-protocol source hashes required by wra-39g2. wra-p06p and wra-39g2 remain implemented but blocked on sudo ./docker/build-appliance.sh and then ./vm-frontend/validate.sh required. tk dep cycle reports no cycles.

**2026-05-18T12:41:35Z**

After Henrik rebuilt the appliance, verified docker/out/artifact-manifest.json is fresh for wra-39g2 (12 source_inputs, including Cargo.toml, Cargo.lock, guest-service sources, and payload-protocol sources). Ran ./vm-frontend/validate.sh required with timeout 300s; it passed. Added validation notes and closed wra-p06p and wra-39g2. tk dep cycle reports no cycles. Epic still has one open child, wra-0djh (guest dmesg/module/service audit), so wra-emj5 remains open.

**2026-05-18T12:45:44Z**

Final closure: all 12 child tickets are closed (including wra-0djh audit), tk dep cycle reports no cycles, and the final required validation after Henrik's appliance rebuild passed in iteration 25. Cleanup goals were met: sync CLI/vmnet wrappers collapsed, QEMU lifecycle consolidated, payload architecture moved to async production paths, guest Docker bridge moved to Rust, stale Python/bubblewrap/workspace compatibility paths removed, vmnet event/buffer helpers consolidated, composed-fs split/deduplicated, guest IPv6 disabled, and appliance source freshness guards added. Residual boot/Docker probe noise is tracked separately as follow-up wra-d02u under wra-piqm and is not a blocker for this epic.
