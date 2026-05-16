---
id: wra-r7of
status: open
deps: []
links: []
created: 2026-05-16T15:50:48Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [frontend, guest, concurrency]
---
# Allocate per-launch payload control ports to avoid cross-project collisions

Problem:
Internal payload control defaults to a fixed host port, so concurrent projects on the same host can collide even though project runtimes are expected to be isolated. Validation also uses fixed published ports.

Relevant code:
- vm-frontend/src/main.rs:3516 and 3524 define/default payload control port behavior.
- docker/runtime-contract.md:288 documents runtime port behavior.
- vm-frontend/validate.sh:160 and nearby live smoke commands use fixed ports.

Impact:
Two different projects launched concurrently can fail due to 127.0.0.1 port conflicts. Validation can be flaky on busy hosts.

Recommended fix:
Allocate and reserve an ephemeral loopback port for internal payload control and plumb the selected port into policy. Keep fixed ports only for explicit user-published ports.

Validation:
- Integration/live test launching two projects concurrently.
- Update validation helpers to allocate free ports for smoke scenarios.

