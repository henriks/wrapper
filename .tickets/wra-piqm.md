---
id: wra-piqm
status: open
deps: []
links: []
created: 2026-05-16T15:50:47Z
type: epic
priority: 1
assignee: Henrik Saksela
tags: [stability, performance, agentvm]
---
# Epic: AgentVM stability and performance hardening pass

Static review across AgentVM's frontend supervisor, vmnet runtime, composed filesystem, guest appliance, and validation flow found several stability and performance risks that should be addressed as a coordinated hardening pass.

Scope:
- vm-frontend launch lifecycle, CLI/TUI/payload control, runtime manifests, and state disk handling.
- vmnet runtime, QEMU stream IO, DNS/TCP/TLS proxying, smoltcp session lifecycle, and host ingress.
- composed-fs namespace/inode model, path traversal, POSIX lock bridge, readdir/cache behavior, and local virtiofsd boundary.
- docker guest init, guest payload server, Docker socket bridge, appliance builder, and validation docs/scripts.

Review source:
- Four parallel component reviews plus a local cross-check performed on 2026-05-16.
- Existing open tickets considered for duplication: wra-y5l6 and wra-ylt9.

Epic acceptance:
- Child tickets are triaged, implemented or explicitly re-scoped, and linked to any existing owner tickets where they overlap.
- Stability fixes include targeted offline tests and any relevant fuzz/stress coverage for arbitrary input/output paths.
- Live behavior changes are exercised with the appropriate live validation tier before closing implementation tickets.
- The required gate remains accurate: ./vm-frontend/validate.sh required, or an explicit documented limitation if live validation cannot be run.

