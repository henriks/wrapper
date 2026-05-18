# Rust Vmnet Policy Model

Ticket: `wra-40vh`

Date: 2026-05-13

## Goal

The Rust frontend owns the guest network boundary. QEMU only sees a stream
socket connected to the frontend; it no longer owns NAT, DNS, or `hostfwd`.

The policy model is implemented in `vm-frontend/src/network_policy.rs` and is
intentionally stricter than QEMU user-mode networking. Unsupported protocols,
dangerous destination ranges, metadata IPs, IPv6, and UDP/443 are denied by
default.

## Config Shape

Top-level model:

```rust
VmnetPolicy {
    assignment: GuestNetwork,
    mtu: u16,
    egress: EgressPolicy,
    protocols: ProtocolPolicy,
    host_listeners: Vec<HostListener>,
    capture: CapturePolicy,
    tls_mitm: TlsMitmPolicy,
}
```

Guest assignment:

- guest IP: `10.0.2.15`
- gateway IP: `10.0.2.2`
- DNS IP: `10.0.2.3`
- prefix length: `24`
- guest MAC: `02:fc:12:34:56:78`
- MTU: `1500`

These match the current wrapper defaults so guest-init and Docker readiness can
migrate incrementally.

## Default Policy

Default egress is deny. Allow rules must be explicit by domain, IP, or later
policy profile.

Always-denied destination ranges:

- `0.0.0.0/8`
- `10.0.0.0/8`
- `100.64.0.0/10`
- `127.0.0.0/8`
- `169.254.0.0/16`
- `169.254.169.254/32`
- `172.16.0.0/12`
- `192.168.0.0/16`
- `224.0.0.0/4`
- `240.0.0.0/4`

Protocol defaults:

- ARP to the configured gateway is allowed.
- IPv4 is the only network layer supported in v1.
- Guest init disables IPv6 for the VM network defaults and selected interface; any IPv6 frame that still reaches vmnet is denied and logged as defense in depth.
- ICMP to the gateway may be allowed for guest diagnostics.
- UDP is denied by default except DNS to the gateway DNS proxy.
- UDP/443 is always denied to prevent QUIC bypass.
- TCP is denied unless egress policy allows the destination.
- TCP/80 is routed through HTTP interception.
- TCP/443 is routed through HTTPS MITM when CA material is configured.
- Unknown EtherTypes and unknown IPv4 protocols are denied and logged.

## CLI Mapping

`--no-net`:

- keeps the guest NIC, ARP, DHCP, and local gateway services alive
- disables guest egress to ordinary host sockets
- preserves frontend control channels needed for Docker/payload readiness
- rejects `--docker-publish`, matching the current wrapper behavior

`--docker-publish HOST:GUEST`:

- becomes a frontend-owned `HostListener` on `127.0.0.1:HOST`
- proxies to the guest IP and `GUEST` over the userspace TCP gateway
- must not be implemented with QEMU `hostfwd`

Docker and payload control:

- are also frontend-owned loopback `HostListener` entries
- are management traffic, not guest egress policy exceptions
- should remain available even when guest egress is disabled

## TLS MITM

`TlsMitmPolicy` carries:

- CA certificate path
- CA private key path
- per-host certificate generation toggle

No default CA is generated implicitly. HTTPS interception should fail closed
when policy requires MITM but CA material is missing.

The CLI exposes this as:

- `--tls-ca-cert PATH`
- `--tls-ca-key PATH`
- `--tls-generate-per-host-certs`

When TCP/443 interception is enabled, the TCP policy denies the connection
unless all three values are configured. This avoids silently downgrading to a
non-inspected TLS tunnel.

The Rust frontend terminates the guest TLS session with per-host certificates
signed by the configured CA, then opens a separate upstream TLS session using
the guest-provided SNI as the upstream server name. Upstream certificate
validation uses the host native root store. Guest TLS sessions without SNI fail
closed because the frontend cannot safely validate the upstream or generate the
right leaf certificate.

HTTPS request logging uses the same summary model as HTTP interception:
method, destination IP/port, Host header, and path. Request/response bodies,
headers other than Host, cookies, authorization values, and decrypted TLS record
contents are not written to the vmnet event log by default.

## Capture

`CapturePolicy` supports optional guest-side pcap capture. Capture sits at the
QEMU stream frame boundary, before ARP/DHCP/DNS/TCP handlers mutate or respond
to frames.

## Follow-Up Contract

Implementation tickets should consume this model directly:

- `wra-iknn`: stream endpoint and pcap capture
- `wra-cz7d`: ARP/DHCP and gateway assignment
- `wra-zqua`: DNS proxy and domain policy hooks
- `wra-p7m4`: TCP gateway and HTTP interception
- `wra-pkhr`: HTTPS MITM and CA handling
- `wra-z9sh`: host listeners for Docker, payload, and published ports
