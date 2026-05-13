---
id: wra-p7m4
status: closed
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

**2026-05-13T15:23:29Z**

Added TcpProxyBridge in vm-frontend/src/tcp_proxy.rs and QEMU stream runtime pump in vm-frontend/src/vmnet_runtime.rs. TcpProxyBridge owns host-side upstream connections, discovers established smoltcp sessions from VmnetGateway, reads guest payloads, applies HTTP/1 parsing/log events for TCP/80, writes guest bytes to the upstream connection, reads available upstream bytes, and sends them back into the guest TCP session. run_qemu_stream_until_eof() now connects QemuFrameIo framing to VmnetGateway and TcpProxyBridge, writing all generated guest frames back with QEMU stream length prefixes. Added in-memory tests for HTTP request/response bridging and a dynamic scripted QEMU stream that pumps SYN -> ARP -> ACK -> HTTP through the runtime without hard-coded TCP server sequence numbers. Remaining work before closing: use this runtime from the real frontend launch path, harden nonblocking/partial IO behavior for real TcpStream, and run/document a VM smoke test.

**2026-05-13T15:26:58Z**

Hardened the real upstream socket path for nonblocking operation: StdTcpConnector now sets TcpStream nonblocking and TCP_NODELAY after connect. Added an ignored-by-default loopback test for that behavior because the normal sandbox blocks local TCP bind; it was run explicitly with escalation and passed. Added pump_proxy_once() in vmnet_runtime so launch code can continue polling upstream sockets and writing guest frames even when QEMU has not sent another frame; added a delayed-response test proving upstream WouldBlock can be followed by a later pump that emits guest frames without additional guest input.

**2026-05-13T15:29:05Z**

Added buffered nonblocking QEMU stream support. QemuFrameIo now has try_read_frame() returning Frame/WouldBlock/Eof without discarding partial length or payload bytes, plus VmnetStreamEndpoint::accept_one_nonblocking(). Added run_qemu_stream_tick() so real launch code can run one nonblocking VM network iteration: read at most one guest frame, process gateway output, and pump upstream proxy readiness even when no guest frame is available. Tests cover preserving partial frame bytes across WouldBlock and retaining subsequent frames in the buffer.

**2026-05-13T15:31:25Z**

Integrated the vmnet runtime with the launch/supervisor surface. FrontendConfig::supervisor_plan() now carries a concrete VmnetRuntimeConfig instead of only a socket path, with default deny-by-default VmnetPolicy, network assignment, connect timeout, and idle sleep. Added serve_vmnet_gateway() as the real blocking entrypoint: bind Unix stream socket, accept QEMU nonblocking stream, build VmnetGateway + StdTcpConnector + TcpProxyBridge, then run nonblocking ticks until EOF. Added a minimal agentvm-frontend vmnet-gateway CLI with --socket, guest network overrides, --allow-ip, --allow-domain, and --allow-public-internet so the runtime can be launched directly by a future supervisor/smoke harness.

**2026-05-13T15:34:03Z**

Corrected the VM smoke prerequisite finding: /dev/kvm is present when checked outside the filesystem sandbox. The remaining smoke blocker is not KVM; it is launch integration. Existing sandbox-wrap still builds QEMU with -netdev user and hostfwd, so running it as-is would validate the legacy path rather than the Rust stream gateway. Updated vm-frontend/vmnet-runtime-validation.md with the corrected KVM finding and the required Rust frontend task ordering for the real smoke.

**2026-05-13T15:42:32Z**

Rust launch integration progressed under wra-9ida: agentvm-frontend prepare now writes real runtime manifests and prints a QEMU stream command against docker/out artifacts. Full VM HTTP smoke is still outstanding; do not close this ticket until agentvm-frontend launch has booted the microvm and an outbound HTTP request has traversed the Rust vmnet gateway.

**2026-05-13T15:43:32Z**

Real prepare validation now generates a stream QEMU command with console=ttyS0,115200n8 and .sandbox/docker-vm/docker-data.raw. This removes two likely false-negative smoke failures before the remaining real VM network validation.

**2026-05-13T15:54:10Z**

Bounded Rust launch reached guest network configuration on eth0 through the Rust frontend QEMU stream path, but no guest-originated HTTP request was run. The vmnet TCP/HTTP ticket still needs an end-to-end HTTP smoke from the booted guest before closure.

**2026-05-13T15:55:32Z**

Second bounded launch confirmed state.json writes for the Rust stream path. Guest console again reached network configuration and service startup before intentional timeout; still no guest HTTP egress request yet.

**2026-05-13T15:58:54Z**

Added persistent vmnet event logging. FrontendConfig now passes .sandbox/docker-vm/run/vmnet-events.log into VmnetRuntimeConfig; serve_vmnet_gateway appends concise TcpProxyEvent summaries for connects, denies, HTTP requests, guest payloads, and upstream payloads without dumping Ethernet frame bodies. A bounded KVM launch created the event log (empty because no guest HTTP was triggered yet).

**2026-05-13T16:03:01Z**

Added a narrow guest HTTP smoke hook that does not depend on host-to-guest ingress. docker/guest-init.sh now reads agentvm_http_smoke_url from /proc/cmdline and, after configuring eth0, runs one wget to that URL with output mirrored to .sandbox/docker-vm/run/guest-http-smoke.log. vm-frontend accepts --guest-http-smoke-url and appends the kernel arg; prepare validation confirmed the generated QEMU command includes agentvm_http_smoke_url=http://93.184.216.34/. Appliance artifacts need a rebuild before the guest-side hook is present in the booted init script.

**2026-05-13T16:12:25Z**

VM smoke passed after appliance rebuild using a deterministic local upstream to avoid external network/sandbox variability. Command: agentvm-frontend launch with --guest-http-smoke-url http://198.51.100.10/ --local-http-smoke-upstream 198.51.100.10:80 --allow-public-internet --qemu-timeout-seconds 30. Guest console reported HTTP smoke request completed; guest-http-smoke.log shows HTTP/1.1 200 OK and saved 2 bytes; vmnet-events.log shows tcp_connected, http_request method=GET host=198.51.100.10 path=/, guest_payload, and upstream_payload for 198.51.100.10:80. QEMU exit was the intentional timeout SIGKILL after validation.
