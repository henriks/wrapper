---
id: wra-xcvq
status: open
deps: []
links: [wra-zqci, wra-jkeg, wra-n0fe, wra-yl7i]
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
