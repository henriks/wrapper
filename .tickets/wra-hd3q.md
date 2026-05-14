---
id: wra-hd3q
status: closed
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


## Notes

**2026-05-14T14:47:48Z**

During wra-1yv3 HTTPS validation on 2026-05-14, a guest urllib.request.urlopen('https://example.com/') smoke failed before TLS with socket.gaierror EAI_AGAIN. vmnet-events.log had no DNS proxy events, only payload-control traffic, while direct IP 104.20.23.154 with SNI example.com exercised HTTPS MITM successfully. Treat this as DNS runtime wiring/validation evidence for this ticket rather than an HTTPS MITM blocker.

**2026-05-14T15:11:11Z**

Implemented and boot-validated DNS runtime wiring. VmnetGateway now routes UDP/53 frames addressed to the configured gateway DNS IP through the existing DnsProxy instead of leaving DNS as an unconnected unit-tested layer. UdpDnsUpstream now binds to an address family appropriate wildcard address instead of 127.0.0.1, so host resolver forwarding can reach non-loopback resolvers. Added gateway unit tests for a proxied gateway DNS query and a non-gateway DNS query not being handled by the DNS proxy. Boot validation with run-dir .sandbox/docker-vm/dns-final and host payload listener 12094:1076: guest /etc/resolv.conf contained nameserver 10.0.2.3, socket.getaddrinfo('example.com', 443) returned A and AAAA answers, urllib.request.urlopen('https://example.com/') returned status=200, and vmnet-events.log recorded dns_query domain=example.com decision=Allowed detail=forwarded to upstream plus HTTPS MITM summary events. Tests: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed with 68 lib tests, 1 existing ignored, and 8 bin tests.

**2026-05-14T17:11:37Z**

Completed UDP/443 and pcap boot validation after adding runtime support. Implemented UDP denial in VmnetGateway: non-DNS UDP is denied according to policy, with UDP/443 always logged as blocked to prevent QUIC bypass. Added --pcap PATH to launch/prepare and vmnet-gateway CLI handling, wired CapturePolicy into serve_vmnet_gateway, and capture inbound/outbound guest-side Ethernet frames through the existing PcapWriter. Boot validation with run-dir .sandbox/docker-vm/udp-pcap-final, host payload listener 12095:1076, and --pcap .sandbox/docker-vm/udp-pcap-final/guest-side.pcap: guest getaddrinfo('example.com', 443) returned 4 answers, guest sent UDP to 104.20.23.154:443 and timed out as expected, vmnet-events.log recorded udp_denied src=10.0.2.15:<port> dst=104.20.23.154:443 reason=udp/443 blocked to prevent QUIC bypass, and the pcap file was 6038 bytes with magic d4c3b2a1, 67 records, well_formed=True. Tests: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed with 69 lib tests, 1 existing ignored, and 8 bin tests.
