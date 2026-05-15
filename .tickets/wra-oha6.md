---
id: wra-oha6
status: open
deps: [wra-wway]
links: []
created: 2026-05-15T09:31:49Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-txoc
---
# Validate mio vmnet runtime and document Tokio path

Complete validation and documentation for the readiness-driven vmnet runtime. Relevant files: vm-frontend/validation-workflow.md, vm-frontend/validation-matrix.md, vm-frontend/network-policy.md if behavior notes are needed, and ticket notes under this epic. Validation should compare the old intent against the new mio runtime for idle CPU behavior, readiness latency, host ingress/published ports, Docker/payload listeners, TCP proxy backpressure, DNS/TCP policy behavior, pcap capture, and event logging. Also document how the new event boundary maps to a possible future Tokio actor driver.

## Design

Use existing fast offline tests as the required baseline. Add focused ignored/live smoke instructions only where QEMU or real host networking is required. The Tokio path should be described as a future driver that feeds the same runtime event model through channels while one actor owns smoltcp; do not propose moving smoltcp behind shared locks.

## Acceptance Criteria

Documentation and/or ticket notes record validation results, known limitations, and rollback/fallback behavior. The docs explain the future Tokio migration path and which modules should remain scheduler-agnostic. cargo test --manifest-path vm-frontend/Cargo.toml --offline and cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check pass. Any relevant live or ignored tests are documented with exact commands and results if run.

