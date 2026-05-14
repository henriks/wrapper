---
id: wra-tad5
status: closed
deps: [wra-k3di]
links: []
created: 2026-05-14T18:40:47Z
type: epic
priority: 1
assignee: Henrik Saksela
tags: [validation, testing, frontend]
---
# Build exhaustive VM frontend validation suite

Create an exhaustive validation program for the VM frontend, covering both major enforcement surfaces: the userspace network gateway and the composed filesystem/runtime sharing layer. This epic exists because recent wrapper work exposed issues that should have been caught earlier: missing wrapper CA bootstrap and silent TLS-stream corruption from partial smoltcp sends. The suite should make these classes of regressions hard to reintroduce.\n\nScope includes fast offline Rust tests, in-process integration tests, model/property-style filesystem tests, live KVM/QEMU self-tests, stress/adversarial tests, and clear documentation for when/how each tier runs.\n\nRelevant code areas: vm-frontend/src/* network gateway/proxy/payload/launch code, composed-fs crate, docker/guest-init.sh, docker/guest-payload-server.py, runtime manifests, and vm-frontend self-test/wrapper paths.

## Acceptance Criteria

An implementation plan exists as child tickets with correct dependencies. The eventual suite covers network, filesystem, wrapper/runtime, and live VM contracts. Each child ticket requires documenting outcomes, coverage gaps, and any new known limitations before close.


## Notes

**2026-05-14T20:05:30Z**

Validation epic completed. Child tickets added the validation matrix, shared harnesses, fast packet/policy/manifest/path tests, TCP/HTTP/HTTPS proxy integration coverage, composed-fs operation model tests, DNS/host-ingress/published-port coverage, virtiofs lifecycle/concurrency coverage, wrapper/runtime contract tests, live KVM self-test expansion, stress/property suites, observability/artifact assertions, and workflow documentation. Verified local fast and stress tiers through vm-frontend/validate.sh fast and vm-frontend/validate.sh stress. User reported the expanded live KVM self-test appeared to pass. Dependency cycle check passed with tk dep cycle.
