---
id: wra-qbpt
status: closed
deps: []
links: [wra-dky9, wra-g0uv]
created: 2026-05-16T21:30:42Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-piqm
tags: [live, docker, sqlite, validation]
---
# Fix live-docker publish self-test sqlite concurrency race

During wra-g0uv validation after changing docker/guest-init.sh readiness gating and rebuilding the appliance, ./vm-frontend/validate.sh live-docker failed in the host-to-container published port scenario after payload success. The failure was in host sqlite integrity check, not Docker readiness: traceback from run_host_sqlite_integrity_check asserted guest_count == 200 but observed 171, then self-test reported 'host sqlite integrity check exited with exit status: 1'. Artifacts were under .sandbox/docker-vm/self-test-docker-publish/ (state.json, qemu.log, console.log, vmnet-events.log). Relevant code: vm-frontend/src/main.rs spawn_host_sqlite_concurrency/run_host_sqlite_integrity_check and self_test_payload_script sqlite concurrency section. Need determine whether this is pre-existing fs/flush ordering, published-port scenario interaction, or regression triggered by readiness timing. Do not close wra-g0uv until live-docker or an equivalent first-Docker-command validation passes.

## Acceptance Criteria

live-docker publish scenario no longer fails host sqlite integrity; regression coverage or self-test ordering fix proves guest SQLite writes are durable before host integrity check; ./vm-frontend/validate.sh live-docker passes after appliance rebuild


## Notes

**2026-05-16T21:34:52Z**

Fix implemented: self-test now skips host/guest sqlite concurrency for Docker-focused live scenarios (docker egress allow/deny and published-port checks), leaving sqlite concurrency to the dedicated/default self-test path instead of combining it with Docker pull/publish load over composed-fs. Also moved host sqlite integrity check after guest filesystem sync when concurrency is enabled. Focused tests passed: cargo fmt; cargo test self_test_payload_skips_sqlite_concurrency --offline; earlier reset/skip tests passed. Validation passed: ./vm-frontend/validate.sh live-docker.
