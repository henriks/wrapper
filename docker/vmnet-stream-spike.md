# QEMU Stream Vmnet Feasibility Spike

Ticket: `wra-lxhx`

Date: 2026-05-13

## Outcome

The local QEMU build supports the stream netdev backend needed for a
rootless userspace virtual network gateway.

Validated locally:

- QEMU version: `10.2.2`
- backend list includes `stream`
- microvm virtio-net device still uses `virtio-net-device,netdev=net0`
- Unix stream syntax requires `addr.type=unix`
- for this project, QEMU should run as the client and the Rust gateway should
  own the Unix listener

Target command shape:

```sh
-netdev stream,id=net0,server=off,addr.type=unix,addr.path=/path/to/vmnet.sock,reconnect-ms=250
-device virtio-net-device,netdev=net0,mac=02:fc:12:34:56:78
```

`server=off` is also the default when `server` is omitted, but spelling it out
is clearer because gateway ownership of the socket is a security boundary.

## Tested Commands

The backend is available:

```sh
qemu-system-x86_64 --version
qemu-system-x86_64 -netdev help
```

Observed:

```text
QEMU emulator version 10.2.2
...
Available netdev backend types:
socket
stream
...
user
...
```

QEMU option help documents Unix stream syntax in the installed manpage:

```text
-netdev stream,id=str[,server=on|off],addr.type=unix,addr.path=path[,abstract=on|off][,tight=on|off][,reconnect-ms=milliseconds]
```

This syntax is accepted by local QEMU:

```sh
qemu-system-x86_64 \
  -nodefaults \
  -display none \
  -machine microvm,acpi=off \
  -monitor none \
  -S \
  -netdev stream,id=net0,server=off,addr.type=unix,addr.path=/tmp/vmnet.sock,reconnect-ms=250 \
  -device virtio-net-device,netdev=net0,mac=02:fc:12:34:56:78
```

Counterexample checked:

```sh
qemu-system-x86_64 ... -netdev stream,id=net0,addr.path=/tmp/vmnet.sock ...
```

Observed error:

```text
Parameter 'addr.type' is missing
```

## Frame Contract

The QEMU stream backend sends one Ethernet frame as:

```text
4 byte big-endian frame length
raw Ethernet frame bytes
```

The upstream QEMU `net/stream.c` implementation constructs a 32-bit length
with `htonl(size)` and writes it immediately before the Ethernet frame bytes.
Incoming traffic is parsed through the same socket read-state machinery and
then delivered to the guest NIC as a packet.

Practical implications for the Rust gateway:

- read exactly four bytes before each guest-originated frame
- reject frames larger than the configured MTU plus L2 overhead
- write the same prefix before gateway-originated Ethernet frames
- treat short reads as normal stream behavior, not packet boundaries
- keep the Unix socket single-client; a second QEMU connection should fail or
  replace only after a controlled teardown

Primary source checked:

- https://qemu.googlesource.com/qemu/+/refs/heads/stable-9.2/net/stream.c

## Lifecycle

For this project the Rust frontend should:

1. create a private runtime directory for the VM
2. bind the vmnet Unix socket before starting QEMU
3. pass `server=off,addr.type=unix,addr.path=...` to QEMU
4. include `reconnect-ms=250` so QEMU survives brief gateway startup races
5. fail the VM startup if the Rust gateway cannot bind the socket

QEMU client mode is important. If QEMU owns the listener with `server=on`, the
network boundary becomes harder to supervise because the Rust gateway is no
longer the process that owns the externally reachable endpoint.

## Current Wrapper Delta

Current microvm networking is built in `DockerVmManager.build_qemu_command()`
in `sandbox-wrap`. It uses:

```text
-netdev user,id=net0,ipv6=off,hostname=agentvm,net=10.0.2.0/24,host=10.0.2.2,dns=10.0.2.3,restrict=...,hostfwd=...
-device virtio-net-device,netdev=net0,mac=...
```

The Rust frontend branch should replace only the netdev backend, not the
virtio-net microvm device:

```text
-netdev stream,id=net0,server=off,addr.type=unix,addr.path=$RUNTIME/vmnet.sock,reconnect-ms=250
-device virtio-net-device,netdev=net0,mac=$GUEST_MAC
```

The kernel command line still needs to pass the guest IP, gateway IP, prefix,
DNS IP, and guest MAC until guest-init moves to DHCP-only configuration. The
DHCP ticket should decide whether to remove those boot args after the gateway
is functional.

## Hostfwd Replacement

Replacing QEMU user networking removes these existing behaviors:

- host access to the guest Docker bridge TCP port
- host access to the guest payload control TCP port
- user-requested `--docker-publish HOST:GUEST` forwards

These must be reimplemented in the Rust frontend, not retained as parallel
QEMU user-mode networking. Ticket `wra-z9sh` is the right place for this work.

Recommended shape:

- the Rust gateway listens on host loopback for Docker and payload control
- accepted host connections are proxied to the guest IP and target guest port
- user-published ports use the same host-listener-to-guest-TCP mechanism
- guest outbound policy remains separate and deny-by-default

## TCP/IP Core Decision

Use `smoltcp` for the gateway TCP state machine rather than hand-rolling TCP.

Reasoning:

- ARP, DHCP, DNS, and ICMP can be staged with small parsers
- TCP termination, retransmission, windows, FIN/RST behavior, and backpressure
  are too easy to get subtly wrong
- the project needs correctness at the boundary more than minimal dependency
  count
- HTTP/HTTPS interception can sit above accepted smoltcp sockets while host
  upstream connections use ordinary `tokio::net::TcpStream`

Early milestones should still keep the stream frame reader/writer independent
from smoltcp. That keeps packet capture and L2 diagnostics testable without a
full network stack.

## Follow-Up Adjustments

- `wra-iknn` should implement the QEMU stream endpoint as a reusable module,
  not a one-off diagnostic binary.
- `wra-cz7d` should keep ARP/DHCP separate from TCP handling so the guest can
  acquire an address before TCP work starts.
- `wra-z9sh` is not optional: it replaces QEMU `hostfwd`, and without it the
  current host-to-guest Docker and payload flows regress.
- Live packet evidence still needs a Rust endpoint and a QEMU launch using the
  current appliance. That belongs in `wra-iknn`; this spike validated command
  shape, framing source, and lifecycle constraints.
