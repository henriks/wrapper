---
id: wra-olu4
status: open
deps: []
links: [wra-t2uv, wra-57z4, wra-i3t9, wra-bbgh, wra-kiv5]
created: 2026-05-16T15:50:47Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-piqm
tags: [vmnet, dns, stability]
---
# Move DNS upstream exchange off the single vmnet event loop

Problem:
DNS upstream lookups are synchronous inside guest frame handling. A slow or blackholed resolver can block the entire vmnet loop for the upstream timeout.

Relevant code:
- vm-frontend/src/vmnet_gateway.rs:463-466 constructs a UDP DNS upstream with a 5 second timeout.
- vm-frontend/src/dns_proxy.rs:105-123 performs a blocking UDP exchange.
- vm-frontend/src/vmnet_gateway.rs handles guest frames synchronously in the main runtime path.

Impact:
One DNS query can stall TCP sessions, host ingress, smoltcp timers, QEMU frame handling, and event logging.

Recommended fix:
Move DNS upstream exchange behind nonblocking/readiness-driven IO or a bounded worker queue with cancellation/timeout. Keep guest-visible behavior deterministic: delayed response, bounded SERVFAIL, or explicit drop after a short policy budget.

Validation:
- Add a fake DNS upstream that blocks/delays and prove unrelated TCP/host-ingress events continue.
- Add timeout regression asserting bounded SERVFAIL or equivalent guest-visible failure.
- Include fuzz/stress coverage if DNS payload/input handling changes.

