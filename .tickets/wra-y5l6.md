---
id: wra-y5l6
status: open
deps: []
links: []
created: 2026-05-15T21:09:57Z
type: feature
priority: 3
assignee: Henrik Saksela
tags: [appliance, distribution, ux]
---
# Single-binary appliance builder rehydration

Capture the idea of redistributing only the main Rust binary while still allowing the appliance artifacts to be created on first use when missing or stale. The binary would embed the appliance builder inputs/assets and rehydrate them into a cache/build directory, rather than requiring a checked-out docker/ tree at runtime. This is not about rewriting the appliance build logic in Rust; Rust should supervise/detect/cache/lock and dump/run the builder it carries.

## Design

Current appliance build lives under docker/: docker/build-appliance.sh sources docker/appliance.env, installs guest assets (guest-init.sh, guest-socket-bridge.py, guest-payload-server.py), downloads Alpine minirootfs and mise, runs apk/mkinitfs in a chroot, builds docker/out/rootfs.raw plus vmlinuz/initrd.img, and writes docker/out/artifact-manifest.json with source_inputs. vm-frontend currently defaults to docker/out/artifact-manifest.json and has freshness checks in vm-frontend/src/main.rs around ensure_appliance_sources_fresh(). Proposed future shape: add an appliance ensure/build flow in the Rust frontend that, when the selected/default manifest is absent or stale, acquires a build lock, expands embedded builder assets into a cache such as ~/.cache/agentvm/appliances/<content-hash>/build, runs the dumped builder script (possibly Python or shell) with explicit user consent/command, verifies outputs, and then launches using the generated manifest. Cache key should include embedded builder version, appliance.env/version pins, guest assets, and relevant build metadata. Keep the UX explicit because the build may require root/sudo, network access, chroot/mount capabilities, mkfs.ext4 or equivalent, and QEMU/KVM is still needed for runtime. Consider a manual command like agentvm appliance build/ensure and optionally an opt-in auto-build flag/env var for launch/self-test. The distributed path should not depend on the repository docker/ directory being present.

## Acceptance Criteria

A design/implementation ticket is complete when there is a concrete plan or implementation for single-binary appliance rehydration: (1) documents host prerequisites and security/consent model; (2) defines cache layout and content-hash invalidation; (3) defines how embedded builder/assets map to artifact-manifest.json source/build metadata; (4) defines CLI UX for appliance build/ensure and launch behavior when artifacts are missing/stale; (5) preserves or intentionally replaces existing docker/build-appliance.sh development workflow; (6) includes tests for missing/stale artifact detection, cache key changes, lock behavior, and clear failure messages. If implementation changes config parsing/serialization/defaults, update vm-frontend/config-json.md and relevant tests. Before closing any implementation work, run ./vm-frontend/validate.sh required, including live-smoke where available.

