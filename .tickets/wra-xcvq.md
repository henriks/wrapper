---
id: wra-xcvq
status: open
deps: []
links: [wra-zqci, wra-jkeg, wra-n0fe, wra-yl7i, wra-7t63]
created: 2026-05-17T10:18:23Z
type: epic
priority: 1
assignee: Henrik Saksela
tags: [refactoring-option-1, tokio, architecture]
---
# Refactoring option 1: idiomatic Rust/Tokio-boundary application architecture

Plan the ruthless refactor path identified by the review: make agentvm a Tokio application at the orchestration and byte-stream I/O boundaries, while keeping single-owner protocol/state cores synchronous and composed-fs/vhost filesystem request execution bounded blocking. Stability and performance are nonfunctional end goals. Backwards compatibility is only relevant for .sandbox/config.json as documented in vm-frontend/config-json.md. Do not preserve command line compatibility shims unless they still serve current product behavior. Key files from review: vm-frontend/src/main.rs, vm-frontend/src/launch.rs, vm-frontend/src/vmnet_runtime.rs, vm-frontend/src/vmnet_poller.rs, vm-frontend/src/vmnet_service_io.rs, vm-frontend/src/payload_client.rs, vm-frontend/src/docker_proxy.rs, guest-service/src/lib.rs, composed-fs/src/lib.rs.

## Design

Target shape: one Tokio runtime at the application edge, an async launch supervisor with owned task lifecycle, async Unix/TCP/process/payload/docker/vmnet edge I/O, synchronous deterministic smoltcp and protocol state, and synchronous composed-fs/vhost filesystem execution with better internal module boundaries and bounded blocking workers. Even if a future vhost transport/control plane becomes async, filesystem request execution should remain blocking worker work unless profiling proves that lock/thread-pool tuning cannot satisfy stability and performance goals. Use tracing and typed errors. Delete duplicated compatibility and pre-Tokio runtime code instead of layering new code over it.

## Acceptance Criteria

This epic is complete when the refactoring option has been either implemented or superseded by a later selected option, all child tickets are closed or explicitly cancelled, and ./vm-frontend/validate.sh required passes in a live-capable environment for any code changes.


## Notes

**2026-05-17T10:20:56Z**

Existing ticket wra-yl7i covers the TUI/backend bridge as a control-socket frontend to an agentvm supervisor. Refactoring option 1 should treat that ticket as the owner of TUI control-plane semantics. Option 1 supervisor/payload tickets must provide clean backend boundaries for wra-yl7i rather than reimplementing TUI coupling.

**2026-05-17T10:28:44Z**

Async-boundary refinement from follow-up three-agent review: option 1 should not aim for an async-all-the-way-down application. The intended target is Tokio at orchestration and byte-stream edges; synchronous single-owner protocol cores; synchronous composed-fs/vhost filesystem execution with bounded blocking workers. Even if the vhost transport/control plane is later rewritten as async, filesystem request execution should remain bounded blocking work unless profiling proves that a deeper async rewrite solves a real bottleneck. Async candidates: supervisor lifecycle, child processes/timeouts, payload TCP, docker proxy, vmnet I/O shell and service workers. Deliberately sync: VmnetGateway/smoltcp state, packet and pcap ordering, policy decisions tied to frame handling, config/manifest/QEMU command construction, composed-fs namespace/handle/lock/filesystem semantics.

**2026-05-17T19:01:39Z**

Continuation-2 iteration 20 final status: closed wra-1esa after required validation; most foundational option-1 Tokio-boundary tickets are closed (workspace/modules/errors/tracing/supervisor/protocol/async payload client/async launch/docker proxy/vmnet core/self-test CLI/CLI compatibility). Remaining option-1 children are wra-662v in_progress, wra-f762 open, wra-yl7i open, and wra-0a0r open. Next recommended priority is resolving wra-662v scope/opt-in Rust appliance validation, then selecting a large remaining boundary ticket deliberately rather than starting broad work at the end of this loop.

**2026-05-17T20:40:15Z**

Closed vmnet poller follow-up wra-avuf after live validation. Production vmnet now uses the Tokio async owner loop with direct service channels, async QEMU stream I/O, timer sleep, and snapshot-wide AsyncFd readiness; RuntimePoller, VmnetServiceWakeup/notifier bridge, and vmnet_poller module were deleted. ./vm-frontend/validate.sh required passed after cleanup. Remaining open option-1 children: wra-662v (blocked on Rust opt-in appliance rebuild/live validation), wra-0a0r, and wra-yl7i.

**2026-05-17T21:29:46Z**

Follow-up wra-7t63 was split from wra-yl7i for payload viewport attach/detach/reconnect semantics. It is linked for traceability but should be treated as post-option-1 follow-up work rather than a child blocker for closing this epic; wra-yl7i closed after required validation for the supervisor-control/TUI-observation boundary.

**2026-05-17T21:32:42Z**

Iteration 51 reflection/checkpoint: all option-1 implementation children except wra-662v are closed and live-validated. wra-7t63 is linked post-option-1 follow-up, not a child blocker. wra-662v has focused Rust guest-service and source-level appliance tests passing plus default required validation, but cannot close under the project live-validation rule because the Rust opt-in appliance path is not rebuilt/exercised. docker/out/artifact-manifest.json remains Python-default (no agentvm_payload_service=rust). Next action requires user-side privileged rebuild: sudo env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/debug/agentvm-guest-service ./docker/build-appliance.sh, then inspect manifest and run live validation; otherwise an explicit scope decision is needed to leave all Rust opt-in live parity/default-switch validation to wra-y335.

**2026-05-17T21:34:53Z**

Iteration 52: rechecked the only remaining child blocker. Rust guest-service binary builds, but docker/out/artifact-manifest.json is still Python-default with no agentvm_payload_service=rust. No further option-1 code work is indicated; epic closure remains blocked on either user-side Rust opt-in appliance rebuild/live validation or an explicit scope-narrowing decision that leaves all Rust opt-in live parity/default-switch validation to wra-y335.

**2026-05-17T21:36:06Z**

Iteration 54: inspected wra-662v acceptance directly. It requires that the appliance can use the Rust guest service in opt-in mode, while wra-y335 remains the parity/default-switch gate. Because the current manifest is not opt-in Rust, closing wra-662v under existing acceptance would violate the live-validation rule. The only honest paths are still: run the Rust opt-in appliance rebuild/live validation, or explicitly narrow wra-662v acceptance to implementation/source/default-required validation and leave appliance opt-in live proof to wra-y335.

**2026-05-17T21:37:11Z**

Iteration 56 reflection: option-1 host/orchestration architecture work is complete and live-validated except for wra-662v's Rust opt-in appliance proof. The Rust guest-service implementation and binary are present, and default required validation has passed previously, but the appliance manifest remains Python-default and this environment cannot run sudo non-interactively. No further code churn should be added to wra-xcvq; next action is external privileged Rust opt-in rebuild/live validation or an explicit scope-narrowing decision leaving appliance parity/default-switch validation to wra-y335.

**2026-05-17T21:38:10Z**

Iteration 58: no Rust opt-in appliance rebuild or explicit scope-narrowing decision was provided after the iteration 57 handoff. Per iteration 56/57 reflections, I did not add source churn or rerun validation against the unchanged Python-default appliance. Epic remains blocked solely on Rust opt-in appliance live validation for wra-662v or an explicit decision to narrow/close wra-662v as implementation-only with wra-y335 carrying live parity/default-switch validation.

**2026-05-17T21:38:35Z**

Iteration 59 near-loop-end status: all option-1 children except wra-662v are closed/live-validated. wra-662v remains in_progress and blocks epic closure under current acceptance. No source churn or validation reruns were performed because the current appliance is still Python-default unless a Rust opt-in rebuild is reported.

**2026-05-17T21:39:01Z**

Iteration 60 final loop status: Ralph loop reached its configured maximum with epic still honestly blocked, not complete. All option-1 host/orchestration children except wra-662v are closed/live-validated. wra-662v remains in_progress because current acceptance requires Rust opt-in appliance usability and docker/out/artifact-manifest.json remains Python-default unless rebuilt with AGENTVM_PAYLOAD_SERVICE=rust. Required external next step: sudo env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/debug/agentvm-guest-service ./docker/build-appliance.sh, inspect manifest for agentvm_payload_service=rust, then run live-payload, live-docker, and required. Alternative: explicitly narrow wra-662v as implementation-only and leave opt-in appliance live proof to wra-y335.
