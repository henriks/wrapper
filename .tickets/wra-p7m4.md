---
id: wra-p7m4
status: in_progress
deps: [wra-zqua, wra-cz7d]
links: []
created: 2026-05-13T10:22:44Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, network, tcp, http, policy]
---
# Implement userspace TCP gateway and HTTP interception

Implement outbound guest TCP handling without kernel NAT. The gateway should terminate guest TCP connections in userspace, preserve the original destination IP/port for policy, open ordinary host TcpStream connections upstream when allowed, and intercept HTTP traffic on port 80 for logging/filtering.

## Design

Use smoltcp or the selected TCP/IP core from the feasibility spike. Do not implement host CAP_NET_ADMIN, TAP, iptables, or kernel NAT. Enforce deny-by-default policy for private ranges, metadata IPs, unknown protocols, and blocked destinations before opening upstream sockets. For TCP/80, parse enough HTTP to log host/path/method/status and apply policy hooks. Non-HTTP TCP behavior should be explicit: either generic connect proxy for allowed destinations or blocked until implemented.

## Acceptance Criteria

Allowed guest TCP connections can reach upstream hosts through ordinary host sockets. Blocked IPs/ranges and unsupported protocols fail closed with logs. HTTP port 80 traffic is intercepted and logged with policy decisions. Tests cover TCP state handling and policy failures; VM smoke validates a simple HTTP request from the guest.


## Notes

**2026-05-13T10:40:46Z**

wra-40vh defines TCP default_action as deny, HTTP port 80 interception enabled by policy, and host listeners as separate management/published-port ingress. TCP gateway should consume VmnetPolicy instead of hard-coding allow/deny behavior.

**2026-05-13T10:59:07Z**

wra-zqua added DNS policy decisions and hickory-proto-based query logging/allow matching. TCP gateway should use DNS decisions/log data as the policy source for destination domains instead of independently parsing DNS or inventing separate allow rules.

**2026-05-13T11:10:34Z**

Partial TCP/HTTP implementation added in vm-frontend/src/tcp_gateway.rs. It uses ipnet for CIDR/range policy checks and httparse for HTTP/1 request parsing. Added TcpDestination/TcpDecision/TcpAction, evaluate_tcp_destination(), parse_http_request(), TcpUpstreamConnector, StdTcpConnector, and connect_if_allowed() so upstream host sockets are opened only after policy allows. Tests cover denied private ranges, denied default egress, HTTP/HTTPS interception decisions, HTTP summary parsing/incomplete/malformed cases, and fail-closed connector behavior. This does not yet implement the smoltcp guest TCP state machine or VM smoke; keep ticket in progress.

**2026-05-13T11:18:51Z**

Added smoltcp v0.13.1 to vm-frontend and introduced vm-frontend/src/guest_tcp.rs as the userspace guest TCP core boundary. GuestTcpCore configures a smoltcp Ethernet Interface on the gateway IP with AnyIP enabled so guest outbound SYN packets retain their original destination IP/port instead of requiring NAT. QueuedEthernetDevice adapts raw Ethernet frames from the QEMU stream framing into smoltcp's Device/RxToken/TxToken model. Tests now cover listener setup, AnyIP/original-destination configuration, and a guest SYN to 93.184.216.34:80 advancing smoltcp to SynReceived, performing ARP neighbor resolution, and emitting a SYN/ACK to the guest MAC. This materially advances TCP state handling, but the ticket remains open until the event loop bridges established smoltcp sessions to TcpUpstreamConnector/HTTP interception and a VM smoke test is documented.

**2026-05-13T11:20:28Z**

Extended the smoltcp TCP core with GuestTcpCore::recv_available() and tests that complete a guest TCP three-way handshake, transition the smoltcp listener to Established, inject an HTTP/1.1 request payload, and read the bytes back from the smoltcp receive buffer. This validates that HTTP interception can consume guest-side TCP payloads from the userspace stack. Remaining work is the production event loop: create/destroy listeners or sessions from observed destinations, connect allowed sessions to TcpUpstreamConnector, feed received bytes through parse_http_request()/policy logging, write upstream/downstream bytes back into smoltcp, and run/document VM smoke.

**2026-05-13T11:21:25Z**

Added parse_tcp_syn_destination() in vm-frontend/src/guest_tcp.rs. It parses raw Ethernet/IPv4/TCP frames with smoltcp wire types before the frame is handed to the TCP stack, returns the original destination IP/port for initial SYN packets, and ignores non-initial segments. This is the event-loop hook needed to policy-check an outbound destination and provision an appropriate smoltcp listener before consuming the SYN; avoids hard-coding only HTTP/HTTPS listeners and avoids NAT-style destination rewriting.

**2026-05-13T11:22:12Z**

Added GuestTcpCore::ensure_listener(port), tracking listener slots by port. It reuses an existing Listen-state socket and provisions a new listener after a prior socket on the same port has accepted/established a connection. Tests cover listener reuse and replenishment after a completed HTTP handshake. This is needed for the production frame loop to policy-check each initial SYN, install an acceptor, and keep accepting subsequent guest connections without adding duplicate legacy paths.

**2026-05-13T11:22:47Z**

Added evaluate_tcp_syn_frame(policy, frame), composing raw SYN destination extraction with evaluate_tcp_destination(). Tests cover allowed HTTP interception and deny-by-default failure before smoltcp accepts the connection. This is the fail-closed policy hook the production loop should call before ensure_listener() and before any host-side TcpStream connect.

**2026-05-13T11:23:07Z**

Added GuestTcpCore::send_to_session() and a test that writes HTTP response bytes into an established smoltcp session, polls the core, and verifies guest-directed Ethernet/TCP payload emission. Together with recv_available(), this gives the future HTTP/proxy loop narrow APIs for both guest->host and host->guest data paths without custom TCP.

**2026-05-13T11:24:22Z**

Added vm-frontend/src/vmnet_gateway.rs as a frame-pump layer for the future QEMU stream loop. VmnetGateway handles L2 replies first, evaluates initial TCP SYN policy before smoltcp, fails denied SYNs closed with no guest frames, provisions smoltcp listeners for allowed destinations, polls the TCP core, and drains generated guest Ethernet frames. Tests cover denied SYN fail-closed behavior and allowed SYN listener installation/polling. This is still not the full async stream runtime or VM smoke, so wra-p7m4 remains in progress.
