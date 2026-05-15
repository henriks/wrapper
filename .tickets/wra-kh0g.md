---
id: wra-kh0g
status: closed
deps: [wra-gx6d, wra-pssg, wra-1bks, wra-0hio]
links: []
created: 2026-05-15T10:37:41Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-neci
---
# Build a named live validation matrix

validate.sh live is a single happy-path VM boot. It caught several issues, but it still does not exercise no-net, hostile, published TCP beyond ping, HTTP/HTTPS MITM, local upstreams, Docker bridge variants, large payload frames, fragmented stdio, or failure artifact assertions.

## Design

Add named live scenarios, likely live-smoke and live-full. Cover public egress, --no-net, --hostile, published TCP/payload listener, Docker bridge, local HTTP upstream allow/deny, DNS allow/deny, UDP/443 denial, HTTPS MITM with guest CA, large payload request/response, fragmented stdout/stderr, and repeated runs. Keep expensive cases opt-in but easy to run. Use local fixtures where possible to reduce dependence on Docker Hub and public DNS.

## Acceptance Criteria

The live validation matrix has multiple named scenarios with clear output and failure diagnostics. At least one required live scenario remains quick. A broader live-full tier covers no-net/hostile/network/Docker/payload/security paths. Tests or docs make clear which tiers require KVM/network/root-built appliance artifacts.


## Notes

**2026-05-15T11:35:23Z**

Started and implemented named live matrix in vm-frontend/validate.sh: live-smoke (also host-live/live, required quick scenario), live-hostile (hostile --no-net scenario), and live-full (runs all named live scenarios). Updated AGENTS.md, vm-frontend/validation-workflow.md, and README. Validation: docs/fmt passed, live-hostile passed, required passed with live-smoke, and live-full passed running live-smoke plus live-hostile.
