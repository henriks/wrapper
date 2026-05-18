---
id: wra-piqm
status: open
deps: []
links: [wra-9m5h, wra-emj5, wra-p06p]
created: 2026-05-16T15:50:47Z
type: epic
priority: 1
assignee: Henrik Saksela
tags: [stability, performance, agentvm]
---
# Epic: AgentVM stability and performance hardening pass

Static review across AgentVM's frontend supervisor, vmnet runtime, composed filesystem, guest appliance, and validation flow found several stability and performance risks that should be addressed as a coordinated hardening pass.

Scope:
- vm-frontend launch lifecycle, CLI/TUI/payload control, runtime manifests, and state disk handling.
- vmnet runtime, QEMU stream IO, DNS/TCP/TLS proxying, smoltcp session lifecycle, and host ingress.
- composed-fs namespace/inode model, path traversal, POSIX lock bridge, readdir/cache behavior, and local virtiofsd boundary.
- docker guest init, guest payload server, Docker socket bridge, appliance builder, and validation docs/scripts.

Review source:
- Four parallel component reviews plus a local cross-check performed on 2026-05-16.
- Existing open tickets considered for duplication: wra-y5l6 and wra-ylt9.

Epic acceptance:
- Child tickets are triaged, implemented or explicitly re-scoped, and linked to any existing owner tickets where they overlap.
- Stability fixes include targeted offline tests and any relevant fuzz/stress coverage for arbitrary input/output paths.
- Live behavior changes are exercised with the appropriate live validation tier before closing implementation tickets.
- The required gate remains accurate: ./vm-frontend/validate.sh required, or an explicit documented limitation if live validation cannot be run.


## Notes

**2026-05-16T21:47:19Z**

Review refresh after the async/payload/vmnet refactors: closed wra-d6vo because wra-35eb/wra-m7gg now provide policy-driven signal handling, nonblocking signal pipe safety, cancellable plain payload sessions, and focused regressions with required validation recorded. Closed wra-olu4 and wra-i3t9 because wra-kiv5 now wires DNS and TCP connect through bounded service workers with owner-side completion handling, guest-visible failure closure, delayed/blocked regressions, and required validation. Kept wra-8fjd, wra-ylfx, wra-57z4, wra-e9sr, and wra-vl1o open with updated notes describing remaining scope after the refactors.

**2026-05-18T05:39:25Z**

Created broader cleanup epic wra-9m5h from the duplicate codebase scans. wra-piqm remains the stability/performance hardening epic; wra-9m5h owns deletion/consolidation pressure across the same areas so hardening tickets should avoid preserving transitional paths when removal is viable.

**2026-05-18T07:23:11Z**

Cleanup epic wra-9m5h iteration 11: wra-2ku5 moved dynamic launch metadata out of kernel cmdline into guest-config/launch.json and added guest/live self-test assertions for the new contract. This aligns with stability hardening by reducing stringly boot protocols. Closure is blocked only by host-side appliance rebuild/required validation in this harness.

**2026-05-18T08:22:38Z**

Cleanup loop reflection iteration 26: wra-bbgh audit/test work indicates the original all-or-nothing QEMU write failure has likely been superseded on the production path by the async owner runtime using tokio QemuFrameIo and write_frame_async().await; new regression coverage proves partial/Pending async writes preserve frame boundaries. Remaining vmnet hardening should decide whether slow QEMU reads require a bounded owner-side output queue for fairness/starvation, while cleanup ticket wra-ynx7 should later delete/quarantine sync frame-pump scaffolding rather than hardening it as a parallel production path.

**2026-05-18T08:48:22Z**

Cleanup loop wra-o75s progress: Python payload server path is being deleted after Rust parity validation. The remaining Python guest-socket-bridge is intentionally not part of this deletion; Docker bridge replacement should stay a separate validated ticket if desired. Appliance builds now require a musl-compatible AGENTVM_GUEST_SERVICE_BIN instead of a PAYLOAD_SERVICE selector.

**2026-05-18T09:17:10Z**

Cleanup loop `wra-ynx7` closed the vmnet cleanup slice after the stability prerequisites (`wra-bbgh`, `wra-e9sr`, `wra-57z4`) landed and `wra-jenv` was superseded. Current vmnet production path has bounded owner-side fd-readiness pumps, owner-side async QEMU writes, centralized TCP reaping, and no stale `allow(dead_code)` vmnet scaffolding. Required validation passed at `/tmp/pi-bash-6e47da3ee3d2cc40.log`.

**2026-05-18T10:13:40Z**

Cleanup epic wra-9m5h / wra-8xsb: composed-fs correctness prerequisites are closed and the structural cleanup preserved the bounded blocking backend. Host traversal/metadata helpers now live in composed-fs/src/host_ops.rs as the openat2-confined host boundary; ComposedFs remains the synchronous bounded FileSystem owner rather than introducing async filesystem core code. Required validation passed at /tmp/pi-bash-993c468a1e57703b.log.

**2026-05-18T10:37:37Z**

Follow-up cleanup epic wra-emj5 was created after three duplicate scans on 2026-05-18. wra-9m5h is closed, but residual cleanup gaps remain: payload sync/async split, sync CLI/vmnet runtime island, Python guest Docker bridge, /workspace compatibility alias, appliance metadata duplication, and large module/test harness concentration. Open wra-piqm tickets should prefer replacing/deleting old behavior over adding wrapper layers.

**2026-05-18T12:02:48Z**

Cleanup loop wra-emj5 iteration 13: wra-b9b1, wra-g34z, and wra-fpy2 are now closed. Guest appliance service ownership is now a single Rust guest-service binary for payload plus Docker bridge; docker/guest-socket-bridge.py is deleted and live-docker/required validation passed. vmnet hardening work should build on next_async_owner_event_with_fd_snapshot and the new shared stream_buffer helpers for bounded pending buffers, rather than reintroducing separate TCP proxy / host ingress buffer mechanics. Active docs/comments were pruned of removed Python payload/frontend and old q35 fallback references.

**2026-05-18T12:12:41Z**

Cleanup loop wra-emj5 iteration 15: appliance cleanup tickets wra-hizo and wra-do0x are closed with required validation. Bubblewrap is no longer part of the appliance pins/install/manifest/docs. Guest project mounting now fails closed for unsupported project paths and no longer preserves /workspace alias/fallback behavior; project workspace is mounted only at the configured absolute project path.

**2026-05-18T12:28:26Z**

Cleanup loop wra-emj5 iteration 19: composed-fs structural follow-up wra-w8ea is closed with required validation. Namespace/node/mount runtime now lives in composed-fs/src/namespace.rs, openat2-confined host operations remain in host_ops.rs, large unit/property tests live in tests.rs, and test/fuzz scaffolding shares test_support helpers instead of duplicate manifest/IO helper implementations. The bounded blocking FileSystem model remains unchanged.

**2026-05-18T12:35:00Z**

Cleanup loop wra-emj5 iteration 21: remaining appliance hardening children are implemented but awaiting rebuild/validation. wra-p06p disables guest IPv6 defaults/interface setup in guest-init while preserving vmnet IPv6 deny/log defense in depth. wra-39g2 makes appliance freshness account for guest-service/payload-protocol source inputs and rejects repo guest-service binaries older than those sources, addressing the stale docker-bridge binary failure found during wra-b9b1. Both require sudo appliance rebuild before closure.
