---
id: wra-xo1i
status: open
deps: [wra-xaf5]
links: [wra-y5l6]
created: 2026-05-16T15:50:48Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [frontend, appliance, stability]
---
# Run appliance source freshness checks for normal launch and self-test

Problem:
Appliance source freshness enforcement is gated to a smoke path instead of normal launch/self-test paths that also depend on the appliance.

Relevant code:
- vm-frontend/src/main.rs:2775 launch argument/config path.
- vm-frontend/src/main.rs:3420 and 3473 freshness check paths.

Impact:
Normal launch can boot stale docker/out artifacts after guest init, payload server, or socket bridge sources change, leading to slow readiness timeouts or protocol mismatches instead of an actionable rebuild error.

Recommended fix:
Run freshness checks for default repo-managed manifests before normal launch/self-test. Define clear behavior for custom manifests.

Validation:
- Unit test that stale required source hash rejects normal launch config path.
- Live smoke proving stale artifacts fail before QEMU boot.
- Coordinate with wra-y5l6 if artifact rehydration changes this path.

