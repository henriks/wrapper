---
id: wra-1yv3
status: open
deps: [wra-pkhr]
links: []
created: 2026-05-13T21:33:38Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, network, tls, validation]
---
# Validate HTTPS MITM with guest-installed CA in booted Rust vmnet

Boot a microvm through agentvm-frontend launch with stream vmnet, configure --tls-ca-cert/--tls-ca-key/--tls-generate-per-host-certs, and validate TCP/443 HTTPS interception with the MITM CA trusted inside the guest image. This relates to vm-frontend/src/tls_mitm.rs, vm-frontend/src/tcp_proxy.rs, vm-frontend/src/tcp_gateway.rs, vm-frontend/src/main.rs, and vm-frontend/network-policy.md. Required context: wra-pkhr implemented the TLS data path and unit tests; wra-pah4 validated HTTP egress, metadata denial, and --no-net but did not install a CA into the guest image. The goal is to document the exact guest-image CA installation step, launch command, vmnet-events.log output, guest-side HTTPS result, and whether logging remains summary-only.

## Acceptance Criteria

A booted guest HTTPS request succeeds only when policy allows it and the guest trusts the configured CA. vmnet-events.log records decrypted HTTP summary events without sensitive body/header logging. A missing/untrusted CA failure mode is documented. Any guest-image or lifecycle blockers have follow-up tickets.

