---
id: wra-olu4
status: closed
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


## Notes

**2026-05-16T17:08:54Z**

wra-kiv5 first slice split DNS proxy behavior into nonblocking owner-side planning and explicit upstream completion. This provides the seam for moving UdpDnsUpstream::exchange off the vmnet loop while preserving existing malformed/blocked/ServFail semantics; not yet wired into VmnetGateway service queues.

**2026-05-16T17:13:46Z**

wra-kiv5 now exposes VmnetGateway::plan_dns_frame / complete_pending_dns_query. This keeps DNS frame synthesis owner-side while allowing allowed queries to become pending service work without calling the upstream resolver in the vmnet loop. Next remaining step for this bug is runtime worker/wakeup wiring and delayed/blackholed resolver regression.

**2026-05-16T17:20:14Z**

Deferred DNS owner path now exists: VmnetGateway::handle_guest_frame_with_deferred_dns can park an allowed DNS query as VmnetPendingDnsQuery and keep processing unrelated TCP frames without invoking the resolver. Runtime worker/wakeup wiring remains to complete the nonblocking DNS fix.

**2026-05-16T17:39:04Z**

Runtime-level DNS service helper now exists under wra-kiv5: allowed DNS can be queued through VmnetServiceOwner, completions can be matched back to pending context, and full command queues fail closed with SERVFAIL. Remaining work is actual worker execution/wakeup integration in serve_vmnet_gateway.

**2026-05-16T17:54:09Z**

serve_vmnet_gateway now has DNS worker plumbing: DNS queries are deferred to a bounded spawned worker and completions wake the owner via ServiceIo before owner-side response synthesis. Remaining wra-kiv5 work includes delayed/blackholed DNS regression and explicit completion/full/shutdown policy before closure.

**2026-05-16T21:46:28Z**

Review after wra-kiv5 async-service refactor: this bug's concrete DNS scope is now complete. DNS requests in serve_vmnet_gateway are deferred through bounded VmnetServiceOwner/spawned DNS worker plumbing, ServiceIo wakeups drain completions back on the owner, full command queues fail closed with SERVFAIL, and delayed/blocked DNS no longer prevents unrelated TCP SYN handling. wra-kiv5 recorded focused DNS/vmnet tests plus required validation passing. Remaining QEMU write-backpressure risk is tracked separately in wra-bbgh rather than this DNS-blocking ticket.
