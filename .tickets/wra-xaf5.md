---
id: wra-xaf5
status: open
deps: []
links: [wra-y5l6]
created: 2026-05-16T15:50:48Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [appliance, build, stability]
---
# Publish appliance artifacts atomically with hashes and sizes

Problem:
The appliance builder writes outputs in place and the manifest does not include artifact hashes/sizes. An interrupted or failed build can leave a mixed old/new artifact set that still looks source-fresh.

Relevant code:
- docker/build-appliance.sh output creation and manifest write paths around lines 77, 176, and 206.
- vm-frontend/src/main.rs:3401 and freshness/manifest checks consume current artifacts.

Impact:
A stale manifest with a new/partial rootfs.raw, vmlinuz, or initrd.img can produce boot hangs that look like runtime bugs.

Recommended fix:
Build into a staging output directory, write manifest last, include artifact hashes and sizes, then atomically publish. Remove or invalidate old manifest at build start.

Validation:
- Test fixture with stale manifest plus corrupted artifact asserting launch/self-test freshness rejects it.
- Link and coordinate with wra-y5l6, which owns broader single-binary appliance builder rehydration.

