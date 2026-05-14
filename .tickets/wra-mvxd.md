---
id: wra-mvxd
status: open
deps: [wra-y325, wra-s8nn]
links: []
created: 2026-05-14T18:43:15Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-tad5
tags: [validation, testing, observability]
---
# Add validation observability and failure artifact assertions

Ensure validation failures are diagnosable. Add tests and implementation improvements where necessary so network and filesystem failures produce useful artifacts.\n\nNetwork artifacts should include vmnet event logs for DNS decisions, TCP policy decisions, TLS MITM setup/failures, upstream failures, partial-send/backpressure counters or events, host ingress events, and pcap capture where requested.\n\nFilesystem artifacts should include manifest summaries, source class/mount IDs in errors, protected path/readonly violations, operation traces for model tests, and guest/host path context without leaking private key material.\n\nLive VM artifacts should include state.json, console.log, guest dockerd/socket-bridge/payload logs, vmnet-events.log, and clear failure messages from self-test payloads.

## Acceptance Criteria

Tests assert key diagnostics are emitted for representative failures. Any new log/event fields are documented. The ticket notes what artifacts to collect when a live VM validation fails.


## Notes

**2026-05-14T18:56:17Z**

Network packet/policy work added GuestFrameOutcome::UnsupportedProtocol and VmnetGatewayEvent::UnsupportedProtocol with log text formatted as unsupported_protocol reason=.... Include this event in artifact/log assertions so unsupported IPv6, unknown ethertypes, unknown IPv4 protocols, and malformed UDP are visibly classified instead of silently appearing as TcpProgress.

**2026-05-14T18:59:53Z**

Filesystem validation added explicit assertions around skipped optional mounts and MITM CA private-key non-exposure. Observability/artifact tests should ensure manifest summaries and config-fs artifacts make it easy to verify cert-only exposure without printing or mounting private key material.
