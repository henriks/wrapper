---
id: wra-hd3q
status: open
deps: [wra-zqua, wra-iknn]
links: []
created: 2026-05-13T21:33:48Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, network, dns, validation, pcap]
---
# Validate DNS, UDP denial, and pcap capture in booted Rust vmnet

Run booted-guest validation for network behavior that is currently covered by unit tests but not by a microvm smoke. This relates to vm-frontend/src/dns_proxy.rs, vm-frontend/src/vmnet_gateway.rs, vm-frontend/src/vmnet_stream.rs, vm-frontend/src/network_policy.rs, and vm-frontend/vmnet-runtime-validation.md. Required context: wra-zqua implemented DNS policy hooks; wra-iknn implemented stream pcap capture; wra-pah4 validated TCP/80 HTTP, metadata denial, and --no-net. The goal is documenting outcomes, not reimplementing duplicate paths.

## Acceptance Criteria

Document a booted guest DNS query that is allowed and logged, a denied DNS/domain case, UDP/443 or another unsupported protocol failing closed, and a pcap capture file containing guest-side frames. Include exact commands, logs, captures, and any blockers as ticket notes or in vmnet-runtime-validation.md.

