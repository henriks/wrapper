# VMNet Runtime Validation

Tickets: `wra-p7m4`, `wra-pah4`, `wra-1yv3`, `wra-hd3q`

Date: 2026-05-13

## Result

The Rust vmnet runtime is implemented and unit/integration tested. The Rust
frontend now has `prepare` and `launch` entrypoints that write composed-fs and
config-fs manifests, start embedded composed-fs/config-fs/vmnet tasks, and build
QEMU with `-netdev stream`.

As of 2026-05-14, a booted `microvm` has validated the Rust stream gateway for
guest network setup, TCP/80 HTTP interception, HTTPS MITM with a guest-trusted
CA, DNS proxying, UDP/443 denial, pcap capture, local upstream mapping,
deny-by-default metadata/private range blocking, explicit `--no-net` blocking,
Docker/payload host listeners, and generic published TCP host ingress.

Observed local prerequisites:

```text
qemu-system-x86_64: /usr/bin/qemu-system-x86_64
kernel: docker/out/vmlinuz
initrd: docker/out/initrd.img
rootfs: docker/out/rootfs.raw
/dev/kvm: present when checked outside the filesystem sandbox
```

The first `/dev/kvm` check was sandbox-limited. The corrected host check was:

```text
crw-rw-rw- 1 root kvm 10, 232 May 13 09:47 /dev/kvm
```

`wra-p7m4` covered the initial runtime implementation. Current end-to-end
validation runs through the Rust frontend and Rust vmnet runtime.

## Checks Run

```sh
cargo test --manifest-path vm-frontend/Cargo.toml --offline
cargo test --manifest-path composed-fs/Cargo.toml --offline
python3 docker/check-qemu-command-shape.py
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- prepare --project /home/hsaksela/ai/wrapper --run-dir /home/hsaksela/ai/wrapper/.sandbox/docker-vm/run --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json --qemu /usr/bin/qemu-system-x86_64 --allow-public-internet
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- launch --project /home/hsaksela/ai/wrapper --run-dir /home/hsaksela/ai/wrapper/.sandbox/docker-vm/run --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json --qemu /usr/bin/qemu-system-x86_64 --allow-public-internet --qemu-timeout-seconds 10
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- prepare --project /home/hsaksela/ai/wrapper --run-dir /home/hsaksela/ai/wrapper/.sandbox/docker-vm/run --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json --qemu /usr/bin/qemu-system-x86_64 --allow-public-internet --guest-http-smoke-url http://93.184.216.34/
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- launch --project /home/hsaksela/ai/wrapper --run-dir /home/hsaksela/ai/wrapper/.sandbox/docker-vm/run --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json --qemu /usr/bin/qemu-system-x86_64 --allow-public-internet --guest-http-smoke-url http://198.51.100.10/ --local-http-smoke-upstream 198.51.100.10:80 --qemu-timeout-seconds 30
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- launch --project /home/hsaksela/ai/wrapper --run-dir /home/hsaksela/ai/wrapper/.sandbox/docker-vm/run --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json --qemu /usr/bin/qemu-system-x86_64 --guest-http-smoke-url http://169.254.169.254/ --qemu-timeout-seconds 30
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- launch --project /home/hsaksela/ai/wrapper --run-dir /home/hsaksela/ai/wrapper/.sandbox/docker-vm/run --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json --qemu /usr/bin/qemu-system-x86_64 --no-net --guest-http-smoke-url http://198.51.100.10/ --local-http-smoke-upstream 198.51.100.10:80 --qemu-timeout-seconds 30
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- launch --project /home/hsaksela/ai/wrapper --run-dir /home/hsaksela/ai/wrapper/.sandbox/docker-vm/run --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json --qemu /usr/bin/qemu-system-x86_64 --no-net --host-payload-listener 12076:1076 --qemu-timeout-seconds 60
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- launch --project /home/hsaksela/ai/wrapper --run-dir /home/hsaksela/ai/wrapper/.sandbox/docker-vm/run --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json --qemu /usr/bin/qemu-system-x86_64 --no-net --host-docker-listener 12375:1075 --qemu-timeout-seconds 75
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- launch --project /home/hsaksela/ai/wrapper --run-dir /home/hsaksela/ai/wrapper/.sandbox/docker-vm/run --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json --qemu /usr/bin/qemu-system-x86_64 --publish 12077:1076 --qemu-timeout-seconds 60
sh -n docker/guest-init.sh
```

Results:

- `vm-frontend`: 65 library tests passed, 8 binary tests passed, 1 ignored.
- `composed-fs`: 17 passed.
- `docker/guest-init.sh` passed shell syntax validation.
- QEMU command shape: `qemu-command-shape-ok`.
- `agentvm-frontend prepare` wrote
  `.sandbox/docker-vm/run/composed-fs-manifest.json`,
  `.sandbox/docker-vm/run/guest-config/composed-binds.json`, and
  `.sandbox/docker-vm/run/config-fs-manifest.json`, then printed a QEMU command
  using `-netdev stream` with no `-netdev user` or `hostfwd`. The generated
  command uses `console=ttyS0,115200n8` and the existing
  `.sandbox/docker-vm/docker-data.raw` data disk path.
- A bounded `agentvm-frontend launch` was run outside the sandbox so QEMU could
  access `/dev/kvm`. The frontend started embedded composed-fs/config-fs and
  QEMU; `console.log` reached guest init lines for loading `virtio_net`,
  configuring `eth0` as `10.0.2.15/24 via 10.0.2.2`, and starting dockerd,
  the Rust Docker bridge, and the Rust payload service. QEMU was intentionally killed after 10
  seconds by `--qemu-timeout-seconds`, so this is a launch/boot smoke, not the
  final HTTP egress smoke.
- The Rust launcher writes `.sandbox/docker-vm/run/state.json`. A follow-up
  bounded launch ended with `status=exited`,
  `network_backend=stream`, `composed_fs=embedded`, and
  `qemu_status=signal: 9 (SIGKILL)` from the intentional timeout. State also
  records the egress policy mode; a `--allow-public-internet` run wrote
  `egress_default_action=AllowPublicInternet` and
  `egress_reason=ExplicitAllowProfile`.
- The launched vmnet gateway creates
  `.sandbox/docker-vm/run/vmnet-events.log` and appends concise TCP/HTTP proxy
  events. The bounded boot smoke created the file but it stayed empty because
  no guest HTTP request was triggered during the timed run.
- `agentvm-frontend prepare --guest-http-smoke-url http://93.184.216.34/`
  writes the smoke URL into `guest-config/launch.json`, which is exposed through
  the `agentvm-config` virtiofs channel. The kernel command line remains limited
  to boot-critical flags.
- After the appliance rebuild, a local-upstream HTTP smoke passed through the
  Rust stream gateway. The guest requested `http://198.51.100.10/`, while
  `--local-http-smoke-upstream 198.51.100.10:80` mapped that destination to a
  frontend-owned loopback HTTP server. `guest-http-smoke.log` shows
  `HTTP/1.1 200 OK`, and `vmnet-events.log` contains `tcp_connected`,
  `http_request`, `guest_payload`, and `upstream_payload` for
  `198.51.100.10:80`.
- A metadata-IP smoke against `http://169.254.169.254/` now fails quickly with
  guest-side `Connection refused`. The gateway emits a TCP reset for denied
  pre-accept SYNs and logs
  `tcp_denied_preaccept dst=169.254.169.254:80 action=Deny reason=destination is in a denied range`.
  Before this fix, the guest connect hung and no vmnet event was logged.
- Explicit `--no-net` is accepted by the Rust frontend. A booted smoke with
  `--no-net`, `--guest-http-smoke-url http://198.51.100.10/`, and a local
  upstream mapping fails closed before opening the upstream. `state.json`
  records `egress_default_action=Deny` and `egress_reason=NoNetFlag`; the guest
  sees `Connection refused`; `vmnet-events.log` records
  `tcp_denied_preaccept dst=198.51.100.10:80 action=Deny reason=destination denied by egress policy`.
- Payload host ingress works through a frontend-owned listener. Launching with
  `--no-net --host-payload-listener 12076:1076`, then sending the payload
  protocol ping frame to `127.0.0.1:12076`, returns frame `K` with payload `ok`.
  `vmnet-events.log` records `host_ingress_opened`,
  `host_ingress_host_payload`, `host_ingress_guest_payload`, and
  `host_ingress_guest_closed` for guest port `1076`.
- Docker host ingress works through the project Unix socket and frontend-owned
  host listener. Launching with
  `--no-net --host-docker-listener 12375:1075`, then running
  `curl --unix-socket .sandbox/docker-vm/run/docker.sock http://docker/_ping`,
  returns Docker `HTTP/1.1 200 OK` with body `OK`. The Rust guest Docker bridge
  port is `1075`, not Docker's conventional `2375`.
- Published TCP ingress works through the same frontend path. Launching with
  `--publish 12077:1076`, then sending the payload protocol ping frame to
  `127.0.0.1:12077`, returns frame `K` with payload `ok`. The vmnet log records
  `purpose=PublishedTcp` events and the generated QEMU command still contains
  no `hostfwd`.

The ignored frontend test is
`tcp_gateway::tests::std_connector_returns_nonblocking_tcp_stream`; it binds
loopback TCP and is skipped in the default sandbox. It was run explicitly with
escalation and passed.

## Covered Without VM Boot

- QEMU stream frame codec, including big-endian frame lengths, oversized
  frames, truncated frames, and pcap output.
- Buffered nonblocking stream reads that preserve partial QEMU frame bytes
  across `WouldBlock`.
- ARP and DHCP fixed-lease L2 gateway behavior.
- DNS policy/proxy behavior.
- TCP destination policy and fail-closed SYN handling before smoltcp accepts a
  guest connection.
- smoltcp TCP state handling through SYN, ARP neighbor resolution, established
  session payload reads, and guest-directed response writes.
- HTTP/1 request parsing for intercepted TCP/80 traffic.
- HTTPS MITM primitives: configured CA loading, fail-closed missing-CA policy,
  per-host certificate generation, guest-side TLS termination, upstream TLS
  encryption/validation using native roots, and decrypted HTTP summary logging
  are unit tested.
- HTTPS MITM boot validation succeeds with the rebuilt image. The frontend
  exposes the configured `--tls-ca-cert` as
  `/run/agentvm-config/mitm-ca.crt`. Because the guest rootfs is mounted
  read-only, guest-init assembles a combined CA bundle at
  `/run/agentvm-ca-bundle.pem` and exports `SSL_CERT_FILE` and
  `REQUESTS_CA_BUNDLE` before starting guest payload services. A guest Python
  TLS request to `104.20.23.154:443` with SNI `example.com` using
  `ssl.create_default_context()` returned `HTTP/1.1 200 OK`. vmnet logged
  summary-only events: `tcp_connected ... action=InterceptHttps`,
  `http_request ... method=GET host=example.com path=/`,
  `tls_upstream_payload ... bytes=80`, `guest_payload ... bytes=94`, and
  `upstream_payload ... bytes=859`.
- HTTPS MITM failure mode is documented: with a pre-CA-hook image, the same
  guest request reached the MITM and failed with
  `CERTIFICATE_VERIFY_FAILED`; vmnet logged `action=InterceptHttps`,
  `tls_handshake_payload`, `tls_upstream_payload`, and
  `tls_mitm_failed ... UnknownCA`.
- DNS/UDP/pcap boot validation succeeds. `VmnetGateway` routes UDP/53 frames
  addressed to the configured gateway DNS IP through `DnsProxy`, and
  `UdpDnsUpstream` forwards through the host resolver. In a booted guest,
  `/etc/resolv.conf` contained `nameserver 10.0.2.3`,
  `socket.getaddrinfo("example.com", 443)` returned A and AAAA answers, and
  `urllib.request.urlopen("https://example.com/")` returned status `200`.
  `vmnet-events.log` recorded `dns_query domain=example.com decision=Allowed
  detail=forwarded to upstream`.
- UDP/443 denial is boot-validated. A guest UDP datagram to
  `104.20.23.154:443` timed out as expected, and `vmnet-events.log` recorded
  `udp_denied ... dst=104.20.23.154:443 reason=udp/443 blocked to prevent QUIC
  bypass`.
- Pcap capture is boot-validated via `--pcap
  .sandbox/docker-vm/udp-pcap-final/guest-side.pcap`. The generated pcap was
  6038 bytes, had magic `d4c3b2a1`, parsed as well-formed, and contained 67
  captured Ethernet records.
- Runtime frame pump from QEMU stream framing to vmnet gateway to TCP proxy
  bridge and back to guest frames.
- Delayed upstream responses via `pump_proxy_once()` without requiring another
  guest frame.
- Launch/supervisor surface: `FrontendConfig::supervisor_plan()` now carries a
  concrete `VmnetRuntimeConfig`, `agentvm-frontend vmnet-gateway` can run the
  gateway task directly, and `agentvm-frontend prepare`/`launch` cover Rust-side
  manifest generation and stream QEMU command construction.

## Smoke Procedure To Run On KVM Host

1. Build the frontend:

   ```sh
   cargo build --manifest-path vm-frontend/Cargo.toml --offline
   cargo build --manifest-path composed-fs/Cargo.toml --offline
   ```

2. Verify artifacts and command shape:

   ```sh
   test -e /dev/kvm
   test -f docker/out/vmlinuz
   test -f docker/out/initrd.img
   test -f docker/out/rootfs.raw
   python3 docker/check-qemu-command-shape.py
   ```

3. Prepare the Rust runtime and confirm the generated command contains stream
   networking, not usernet:

   ```sh
   cargo run --manifest-path vm-frontend/Cargo.toml -- \
     prepare \
     --project "$PWD" \
     --run-dir "$PWD/.sandbox/docker-vm/run" \
     --artifact-manifest "$PWD/docker/out/artifact-manifest.json" \
     --qemu /usr/bin/qemu-system-x86_64 \
     --allow-public-internet
   ```

4. Rebuild the appliance if `docker/guest-init.sh` changed since the last
   artifact build:

   ```sh
   sudo docker/build-appliance.sh
   ```

5. Launch the Rust frontend. To avoid depending on external network reachability
   from the execution environment, use the local-upstream smoke mapping:

   ```sh
   cargo run --manifest-path vm-frontend/Cargo.toml -- \
     launch \
     --project "$PWD" \
     --run-dir "$PWD/.sandbox/docker-vm/run" \
     --artifact-manifest "$PWD/docker/out/artifact-manifest.json" \
     --qemu /usr/bin/qemu-system-x86_64 \
     --allow-public-internet \
     --guest-http-smoke-url http://198.51.100.10/ \
     --local-http-smoke-upstream 198.51.100.10:80 \
     --qemu-timeout-seconds 30
   ```

   The QEMU command must contain:

   ```text
   -netdev stream,id=net0,server=off,addr.type=unix,addr.path=.sandbox/docker-vm/run/vmnet.sock,reconnect-ms=250
   -device virtio-net-device,netdev=net0,mac=02:fc:12:34:56:78
   ```

6. The rebuilt guest should issue the smoke request automatically. If manually
   testing from the guest instead, run:

   ```sh
   wget -S -O- http://198.51.100.10/
   ```

7. Confirm these outcomes:

   - The guest receives DHCP/ARP/network configuration or the static cmdline
     network setup reaches the gateway.
   - The gateway logs/observes a TCP/80 HTTP request event.
   - `.sandbox/docker-vm/run/guest-http-smoke.log` contains the guest-side
     `wget` result for the automatic smoke.
   - `.sandbox/docker-vm/run/vmnet-events.log` contains an `http_request`
     entry.
   - The host opens the upstream connection only after policy allows it.
   - The guest receives the upstream HTTP response.
   - A blocked destination, for example `169.254.169.254`, fails closed with
     guest-visible connection refusal and a `tcp_denied_preaccept` vmnet event.

## Remaining Gaps

Bounded validation timeouts now have explicit lifecycle reporting. A
`--qemu-timeout-seconds 10` run exits with
`qemu timed out after 10 seconds and was terminated with status: signal: 9 (SIGKILL)`;
`state.json` records `status=timed_out`, and expected embedded backend
disconnects after QEMU termination are suppressed instead of being printed as
misleading composed-fs/config-fs/vmnet failures.

## Readiness-Driven Runtime

As of `wra-txoc`, the launched vmnet gateway uses a narrow `mio` poller instead
of a fixed idle sleep as the normal progress mechanism. The poller owns QEMU
stream, host listener/session, and upstream session fd registration and maps raw
readiness back into domain events. `VmnetGateway` and `GuestTcpCore` remain
synchronous and single-owner; smoltcp is advanced explicitly when guest frames
arrive, app data is sent, sessions close, or the smoltcp poll deadline expires.

The current offline validation command was:

```sh
cargo test --manifest-path vm-frontend/Cargo.toml --offline
```

It covers the runtime poller registration boundary, readiness-buffered host
ingress writes, readiness-buffered upstream proxy writes, QEMU stream framing,
policy behavior, pcap output, and event-log formatting. Live KVM validation
should still be run before relying on this for long interactive sessions because
the command sandbox cannot exercise the real QEMU stream fd, loopback listeners,
or OS readiness timing.

The future Tokio path should keep the same ownership rule: one actor owns
smoltcp and receives QEMU/host/upstream events over channels. Tokio tasks may
replace the `mio` driver as event sources, but smoltcp should not be shared
behind locks or awaited across mutable gateway state.

Additional booted validation still needed before replacement:

- Long-running lifecycle behavior under representative interactive workloads.
