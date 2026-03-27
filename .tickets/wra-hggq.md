---
id: wra-hggq
status: open
deps: []
links: []
created: 2026-03-27T21:22:04Z
type: epic
priority: 1
assignee: Henrik Saksela
tags: [docker, vm, sandbox]
---
# Add project-scoped Docker VM support to sandbox-wrap

Replace direct host Docker socket mounting behind --docker with a project-local VM appliance model. The VM must be managed as sandbox state, expose a host Unix socket for the agent, and use Cloud Hypervisor plus a minimal Debian guest as described in docker.md.

Scope for v1:
- --docker means VM-backed Docker only
- VM runtime state is project-specific under .sandbox/docker-vm/
- The sparse Docker data disk lives under .sandbox/docker-vm/
- The VM starts during sandbox startup for a --docker run
- The VM terminates when that sandbox exits
- --reset removes VM state, sparse disk, sockets, and metadata
- Initial release is Linux + KVM only

## Design

Runtime state root: .sandbox/docker-vm/

Required assets in that root:
- docker-data.raw
- runtime sockets
- PID/state files
- VM metadata

Immutable appliance artifacts may live under docker/ in-repo as build outputs and inputs, but no mutable runtime state may live there.

The VM lifecycle is coupled to a single sandbox process: start during sandbox startup, terminate when that sandbox exits, remove on --reset.

## Acceptance Criteria

Child tickets cover appliance build, host VM manager, socket proxying, sandbox CLI integration, reset/cleanup, and documentation.

Dependencies must reflect implementation order.

