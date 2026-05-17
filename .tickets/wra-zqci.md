---
id: wra-zqci
status: open
deps: [wra-n0fe]
links: [wra-jkeg, wra-xcvq, wra-n0fe, wra-yl7i]
created: 2026-05-17T10:19:11Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, supervisor, process]
---
# Option 1: replace launch process handling and readiness with async supervision

Replace start_frontend_with_policy and RunningFrontend lifecycle with async process and readiness supervision. Current launch.rs starts composed-fs, config-fs, vmnet, docker proxy, and QEMU, but only owns QEMU. Service failures currently mostly eprintln from detached threads.

## Design

Use tokio::process::Command for QEMU and host tools such as mkfs.ext4. Replace wait-timeout with tokio::time::timeout. Replace wait_for_path sleep polling with readiness oneshot channels and concrete probes. Treat vmnet/docker/composed-fs task failure as fatal while QEMU is running, terminate QEMU, write state, and return a typed error. Normal shutdown should be explicit async shutdown; Drop should only be last-resort cleanup.

## Acceptance Criteria

Supervisor owns and joins all launch services, QEMU timeout and service-failure paths are tested, readiness no longer depends on socket path existence alone, and state.json reflects shutdown causes.


## Notes

**2026-05-17T10:20:56Z**

Coordinate with wra-yl7i. Async launch supervision should expose lifecycle state and task outcomes in a form usable by a control-socket frontend. Avoid baking plain CLI or TUI behavior directly into supervisor internals; CLI/TUI should become clients of the same supervisor/control boundary.

**2026-05-17T10:28:44Z**

Async-boundary refinement: async launch supervision should integrate composed-fs as a supervised blocking service. Readiness, failure propagation, cancellation, and state reporting belong in the Tokio supervisor; composed-fs request execution remains synchronous/bounded blocking. If vhost transport later becomes async, keep FS operations behind a bounded blocking worker model unless profiling justifies more.
