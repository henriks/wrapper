# Agent VM Rust Frontend

This crate is the Rust supervisor scaffold for the VM-only sandbox path.

Current scope:

- structured runtime paths for `.sandbox/docker-vm/run`
- microvm QEMU command construction for the validated composed-fs machine shape
- QEMU `stream` netdev command construction for the rootless userspace gateway
- reusable QEMU stream frame codec and optional pcap writer
- initial L2 IPv4 gateway handlers for ARP and DHCPv4 fixed leases
- DNS proxy policy hooks backed by `hickory-proto`
- TCP destination policy and HTTP/1 request parsing backed by `ipnet` and
  `httparse`
- embedded composed-fs server configuration via `agentvm_composed_fs::ServeConfig`
- state snapshot fields for the future `state.json` writer

It deliberately does not keep the Python/QEMU user-networking path alive.
Host-to-guest Docker, payload, and published-port access must be implemented as
frontend-owned listeners over the userspace vmnet gateway.

The vmnet stream layer is in `src/vmnet_stream.rs`. It decodes and encodes the
QEMU wire format as a 4-byte big-endian Ethernet frame length followed by raw
Ethernet bytes, rejects malformed/truncated frames, and can write standard
Ethernet pcap files for guest-side capture.

The first gateway protocol layer is in `src/l2_gateway.rs`. It replies to ARP
requests for the gateway IP and generates DHCPv4 offer/ack responses for the
fixed guest lease. DNS, TCP, HTTP, and HTTPS handlers are intentionally still
future layers over the same decoded Ethernet frames.

DNS policy is in `src/dns_proxy.rs`. It parses and serializes DNS messages with
`hickory-proto`, forwards allowed queries through a host UDP resolver, returns
deterministic `REFUSED`/`SERVFAIL` responses for policy or upstream failures,
and denies attempts to use non-gateway DNS destinations.

TCP policy and HTTP interception helpers are in `src/tcp_gateway.rs`. They do
CIDR-aware allow/deny checks before any host socket is opened, identify
HTTP/HTTPS interception decisions, expose a host `TcpStream` connector boundary,
and parse HTTP/1 request method/path/host with `httparse`. The full guest TCP
state machine is still future work.

Validate:

```sh
cargo test --manifest-path vm-frontend/Cargo.toml --offline
```
