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
- configured-CA HTTPS MITM primitives backed by `rcgen` and `rustls`
- smoltcp-backed userspace TCP handling and a QEMU stream runtime pump for the
  rootless vmnet gateway
- Rust runtime preparation and launch entrypoints that write composed-fs/config
  manifests and start QEMU with stream networking
- embedded composed-fs server configuration via `agentvm_composed_fs::ServeConfig`
- state snapshot fields for the future `state.json` writer

It deliberately does not keep the Python/QEMU user-networking path alive.
Host-to-guest Docker, payload, and published-port access are frontend-owned
listeners over the userspace vmnet gateway, not QEMU `hostfwd` rules.

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
and parse HTTP/1 request method/path/host with `httparse`.

The guest TCP stack and stream runtime are in `src/guest_tcp.rs`,
`src/vmnet_gateway.rs`, `src/tcp_proxy.rs`, `src/host_ingress.rs`,
`src/vmnet_poller.rs`, and `src/vmnet_runtime.rs`. They use `smoltcp` to
terminate guest TCP sessions in userspace, preserve original destinations for
policy, bridge allowed sessions to ordinary host sockets, and write upstream
bytes back as guest Ethernet frames. The launched gateway uses a narrow `mio`
poller for QEMU stream, host listener/session, and upstream socket readiness
while keeping smoltcp owned by one synchronous runtime owner. The gateway writes
concise TCP/HTTP event summaries to `.sandbox/docker-vm/run/vmnet-events.log`.
Denied pre-accept TCP SYNs emit guest-visible resets and `tcp_denied_preaccept`
events so blocked destinations fail closed without hanging guest connects.

Real VM validation status and the KVM-host smoke procedure are documented in
`vmnet-runtime-validation.md`.

The broader network and filesystem validation taxonomy is documented in
`validation-matrix.md`. Exact tier commands and triage guidance are in
`validation-workflow.md`; the shortcut runner is `vm-frontend/validate.sh`.
Coverage-guided fuzz targets and seed corpora live in `vm-frontend/fuzz/`.

Run the KVM-required VM-only self-test:

```sh
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- \
  self-test \
  --project "$PWD" \
  --run-dir "$PWD/.sandbox/docker-vm/self-test" \
  --artifact-manifest "$PWD/docker/out/artifact-manifest.json" \
  --qemu /usr/bin/qemu-system-x86_64 \
  --publish-payload-port 12079
```

This boots the real microvm path, verifies the guest payload control channel,
checks optional host-published guest access by pinging the payload service
through `--publish-payload-port`, runs a trivial payload inside the guest,
checks `$HOME` and workspace sharing, verifies the guest-visible MITM CA bundle
and cert-only config filesystem, confirms config FS is read-only, performs a
guest DNS lookup when network is enabled, verifies `dockerd` with
`docker version` and `docker info`, then runs
`docker run --rm -v "$PWD:/work:ro" alpine:3.22` and reads a workspace file
from inside that container. The image is pulled into the project-local Docker
data disk if it is not already present.

When validation fails, collect the run directory shown in `state.json`. The
state file records the primary diagnostics: `qemu.log`, `console.log`,
`vmnet-events.log`, the composed/config filesystem manifests, and the guest bind
manifest. Event logs include DNS decisions, UDP denials, unsupported protocol
classification, TCP policy/setup failures, TLS MITM failures, and host-ingress
errors without logging private key material.

Prepare manifests and print the Rust stream QEMU command:

```sh
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- \
  prepare \
  --project "$PWD" \
  --run-dir "$PWD/.sandbox/docker-vm/run" \
  --artifact-manifest "$PWD/docker/out/artifact-manifest.json" \
  --qemu /usr/bin/qemu-system-x86_64
```

The blocking launcher entrypoint is:

```sh
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- \
  launch \
  --project "$PWD" \
  --run-dir "$PWD/.sandbox/docker-vm/run" \
  --artifact-manifest "$PWD/docker/out/artifact-manifest.json" \
  --qemu /usr/bin/qemu-system-x86_64 \
  --allow-public-internet
```

Use `--no-net` for an explicit deny-egress launch. Docker and payload control
listeners may still be exposed through frontend-owned listeners, but published
guest ports are rejected with `--no-net`.

For a VM egress smoke after rebuilding the appliance with the current
`docker/guest-init.sh`, add:

```sh
  --guest-http-smoke-url http://93.184.216.34/
```

The guest will run one `wget` after configuring `eth0`, write the guest-side
result to `.sandbox/docker-vm/run/guest-http-smoke.log`, and the vmnet gateway
should log the intercepted request in `.sandbox/docker-vm/run/vmnet-events.log`.

The launcher is intentionally Rust-only for the stream path: it does not add a
QEMU `user` netdev or `hostfwd` fallback.

Validate the full required gate on a KVM-capable host:

```sh
vm-frontend/validate.sh required
```

For fast offline iteration only:

```sh
vm-frontend/validate.sh fast
```

The unit suite is the non-KVM coverage for command parsing, wrapper flag
translation, manifest generation, network policy, payload framing, and self-test
script construction. The required gate also checks formatting, offline guest
service tests, fuzz target compilation, validation documentation drift, and the
quick `live-smoke` host self-test. Broader KVM scenarios are available through
`vm-frontend/validate.sh live-hostile`, `vm-frontend/validate.sh live-payload`,
`vm-frontend/validate.sh live-dns`, `vm-frontend/validate.sh live-docker`,
`vm-frontend/validate.sh live-fs`, and `vm-frontend/validate.sh live-full`.
