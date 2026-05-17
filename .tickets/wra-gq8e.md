---
id: wra-gq8e
status: closed
deps: [wra-n0fe]
links: []
created: 2026-05-17T10:19:17Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, docker, proxy]
---
# Option 1: rewrite docker proxy as supervised Tokio service

Rewrite vm-frontend/src/docker_proxy.rs from a detached thread-per-connection bridge into a small supervised async service. This is a clean Tokio win and should happen before the vmnet migration.

## Design

Use tokio::net::UnixListener, tokio::net::TcpStream, tokio::io::copy_bidirectional, cancellation token integration, typed errors, and a bounded connection limit. Remove per-client threads and duplicated stale socket cleanup. Integrate with the launch supervisor so failure and shutdown are visible.

## Acceptance Criteria

Docker proxy has async unit/integration tests using a fake Unix client and fake TCP upstream, supports cancellation, propagates bind/connect/copy failures through the supervisor, and no longer spawns detached copy threads.


## Notes

**2026-05-17T17:55:05Z**

Continuation-2 iteration 3: started wra-gq8e while wra-662v is blocked on appliance freshness. Added an async Docker Unix proxy implementation alongside the existing sync thread-based entrypoint: run_docker_unix_proxy_async, DockerUnixProxyLimits, Tokio UnixListener/TcpStream bridge using copy_bidirectional, watch-channel shutdown, JoinSet client ownership, and a bounded max_connections semaphore. The sync production launch path is not switched yet. Added focused async tests for Unix-to-TCP bridging, shutdown cancelling open clients, and rejecting zero max_connections. Focused validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline docker_proxy -- --nocapture.

**2026-05-17T17:59:25Z**

Continuation-2 iteration 4: wired the async Docker proxy into the async launch/supervision path. The async launch plan now adds an optional DockerProxy managed task when the vmnet policy exposes a Docker API listener; the LaunchSupervisor has DockerProxy/AsyncDockerProxy task metadata; run_frontend_until_qemu_exit_with_policy_and_timeout_async starts Docker proxy through spawn_supervised_async_service_until_ready instead of the detached sync proxy. Supervised services now carry an optional watch shutdown channel, so QEMU exit/timeout or peer failure signals async services to stop while still preserving bounded-blocking composed-fs/config-fs/vmnet services. Added regression coverage that QEMU exit sends shutdown to an async supervised service, plus existing Docker proxy bridge/shutdown tests. Validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline docker_proxy -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervised_qemu_exit_signals_async_service_shutdown -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervisor -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check. Sync/non-async production launch path still uses the existing thread-based Docker proxy, so wra-gq8e remains in progress until that path is either routed through async launch or has an explicit deletion/compatibility decision.

**2026-05-17T18:01:17Z**

Continuation-2 iteration 5 reflection and cleanup: removed the legacy sync Docker proxy's per-client/threaded copy implementation. start_docker_unix_proxy now binds synchronously for early socket errors, then runs the shared async proxy implementation on a small current-thread Tokio runtime in its service thread; all client bridging uses Tokio tasks and copy_bidirectional. This reduces the side-by-side sync/async duplication while preserving the non-async launch entrypoint. Required validation/closure is still blocked by stale appliance artifacts from wra-662v, but focused and broad vm-frontend validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline docker_proxy -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline supervisor -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline; cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check. Remaining before closing after live-capable validation: decide whether the sync start_docker_unix_proxy service-thread wrapper is acceptable for non-async launch/TUI paths, or route those paths through async launch in a later ticket.

**2026-05-17T18:02:20Z**

Continuation-2 iteration 6 scope decision while appliance rebuild is pending: the remaining start_docker_unix_proxy service-thread wrapper is intentionally kept for legacy non-async launch/TUI/payload paths, but it now reuses the shared Tokio proxy and no longer has detached per-client copy threads. Full routing of those legacy launch paths through the async supervisor should be handled by the broader launch/TUI control-socket work rather than adding another compatibility layer here. wra-gq8e should remain open until required live-capable validation is possible, but the implementation is otherwise ready for final validation.

**2026-05-17T18:21:25Z**

Continuation-2 iteration 11 broad offline validation after the async Docker proxy and vmnet boundary changes passed: cargo test --workspace --offline; cargo fmt --all -- --check. Ticket remains open only because required/live-capable validation is blocked by stale appliance artifacts.

**2026-05-17T18:29:49Z**

Continuation-2 iteration 12: after appliance rebuild, ./vm-frontend/validate.sh required passed, so the workspace/live required gate is no longer blocked by stale artifacts. I am leaving this ticket open for now because the required tier's live scenarios do not appear to exercise the Docker proxy path itself; before closure, run a live Docker-proxy scenario such as ./vm-frontend/validate.sh live-docker or an equivalent --docker launch smoke so the async Docker proxy code is live-exercised.

**2026-05-17T18:32:20Z**

Continuation-2 iteration 13: Docker-specific live validation passed after the default appliance rebuild: ./vm-frontend/validate.sh live-docker. The scenario covered container egress allow, no-net Docker egress denial, and host-to-container published-port access. This exercises the Docker socket bridge/proxy path in a live QEMU/KVM appliance; the implementation has no detached per-client copy threads and reuses the shared Tokio proxy from both async supervision and the legacy service-thread wrapper. Required validation also passed in iteration 12. Closing wra-gq8e.
