---
id: wra-rjt4
status: closed
deps: [wra-r070]
links: []
created: 2026-05-17T10:18:59Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, observability]
---
# Option 1: add tracing for launch and runtime supervision

Add structured observability before replacing runtime internals. Current launch/runtime paths rely on println phase strings and eprintln warnings, which makes async supervision failures hard to diagnose.

## Design

Introduce tracing spans/events around command dispatch, runtime paths, socket paths, QEMU PID/status, service task start/readiness/failure/shutdown, policy decisions, payload lifecycle, and vmnet events. Keep user-facing CLI output intentional and separate from diagnostic logs.

## Acceptance Criteria

Launch and runtime services emit structured task and lifecycle events, failures include service names and causes, and tests or snapshots are updated where output changes.


## Notes

**2026-05-17T12:11:40Z**

Starting after wra-r070 closure. Plan: first establish minimal structured tracing infrastructure at vm-frontend command/launch/runtime boundaries without changing intentional CLI phase output, then replace diagnostic eprintln warnings/failures with tracing events where tests should not observe output changes.

**2026-05-17T12:14:14Z**

Iteration 23 initial tracing slice: added direct workspace tracing dependency for vm-frontend and instrumented launch.rs with structured spans/events around frontend launch startup, service readiness, service thread failures, QEMU spawn/PID/wait/exit/timeout, termination, and unfinished-drop cleanup. Replaced diagnostic service-thread eprintln failures with tracing error events; intentional CLI phase stdout remains unchanged. Validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo check --manifest-path vm-frontend/Cargo.toml --offline; cargo test --manifest-path vm-frontend/Cargo.toml --offline launch_ -- --nocapture.

**2026-05-17T12:16:02Z**

Iteration 24 tracing slice: added structured command-dispatch and launch CLI lifecycle events (configured launch span, local smoke upstream, payload listener readiness, payload execution mode/exit code, qemu success/timeout/failure). Added docker unix proxy tracing for service start, accepted clients, upstream connects, and client/accept failures. Kept intentional launch phase stdout and existing guest-flush warning stderr unchanged while also emitting tracing warnings. Validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline frontend_ -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline wrapper_ -- --nocapture.

**2026-05-17T12:19:01Z**

Iteration 25 tracing slice: instrumented vmnet runtime boundaries and event-log seams. serve_vmnet_gateway now creates a vmnet.runtime span and emits lifecycle events for socket bind/accept, host listener binding, and QEMU stream close/EOF. Gateway/proxy/host-ingress events now also emit tracing debug/warn events (warn for denials/failures/limits) independently of whether the text event log file is enabled. Added a minimal env-controlled binary subscriber (AGENTVM_LOG=error|warn|info|debug|trace) so diagnostics are available without adding tracing-subscriber/offline dependency churn; default CLI output remains unchanged when AGENTVM_LOG is unset. Validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_ -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline argv0_no_longer_selects_wrapper_or_tool_behavior -- --nocapture; manual AGENTVM_LOG=info target/debug/agentvm-frontend --help emitted a command trace on stderr while preserving usage stdout.

**2026-05-17T12:19:37Z**

Iteration 26 reflection: wra-rjt4 now has structured tracing at command dispatch, launch supervision, launch CLI payload lifecycle, docker proxy, and vmnet runtime/event-log seams, with AGENTVM_LOG opt-in stderr diagnostics and default CLI output unchanged. The small boundary-first slices are working: focused tests and full vm-frontend offline tests remain green. Main tradeoff is the deliberately minimal in-tree subscriber instead of tracing-subscriber; this avoids offline dependency churn but is less featureful, which is acceptable for current diagnostics and can be revisited later. Broad offline validation passed: cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check and cargo test --manifest-path vm-frontend/Cargo.toml --offline. Next step is required live-capable validation; if it passes with no appliance rebuild request, close wra-rjt4.

**2026-05-17T12:22:01Z**

Required live-capable validation for tracing slice passed: ./vm-frontend/validate.sh required completed successfully, including docs/fmt/offline tests, fuzz target compilation, live-smoke, and live-setup-tools. No appliance rebuild was requested. wra-rjt4 acceptance covered by structured tracing at command dispatch, launch supervision, launch CLI payload lifecycle, docker proxy, and vmnet runtime/event-log seams, with opt-in AGENTVM_LOG diagnostics and default CLI output unchanged.
