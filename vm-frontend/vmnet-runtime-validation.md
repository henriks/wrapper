# VMNet Runtime Validation

Ticket: `wra-p7m4`

Date: 2026-05-13

## Result

The Rust vmnet runtime is implemented and unit/integration tested. The Rust
frontend now has `prepare` and `launch` entrypoints that write composed-fs and
config-fs manifests, start embedded composed-fs/config-fs/vmnet tasks, and build
QEMU with `-netdev stream`. A real microvm HTTP smoke has still not been run
from this frontend.

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

Do not close `wra-p7m4` until the Rust vmnet runtime is exercised by a booted
`microvm`, not merely by the existing Python launcher’s user-mode networking.

## Checks Run

```sh
cargo test --manifest-path vm-frontend/Cargo.toml --offline
cargo test --manifest-path composed-fs/Cargo.toml --offline
python3 docker/check-qemu-command-shape.py
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- prepare --project /home/hsaksela/ai/wrapper --run-dir /home/hsaksela/ai/wrapper/.sandbox/docker-vm/run --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json --qemu /usr/bin/qemu-system-x86_64 --allow-public-internet
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- launch --project /home/hsaksela/ai/wrapper --run-dir /home/hsaksela/ai/wrapper/.sandbox/docker-vm/run --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json --qemu /usr/bin/qemu-system-x86_64 --allow-public-internet --qemu-timeout-seconds 10
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- prepare --project /home/hsaksela/ai/wrapper --run-dir /home/hsaksela/ai/wrapper/.sandbox/docker-vm/run --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json --qemu /usr/bin/qemu-system-x86_64 --allow-public-internet --guest-http-smoke-url http://93.184.216.34/
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- launch --project /home/hsaksela/ai/wrapper --run-dir /home/hsaksela/ai/wrapper/.sandbox/docker-vm/run --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json --qemu /usr/bin/qemu-system-x86_64 --allow-public-internet --guest-http-smoke-url http://198.51.100.10/ --local-http-smoke-upstream 198.51.100.10:80 --qemu-timeout-seconds 30
```

Results:

- `vm-frontend`: 51 library tests passed, 3 binary tests passed, 1 ignored.
- `composed-fs`: 17 passed.
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
  socket bridge, and payload server. QEMU was intentionally killed after 10
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
  appends `agentvm_http_smoke_url=http://93.184.216.34/` to the generated
  kernel command line. The current boot artifacts must be rebuilt from the
  updated `docker/guest-init.sh` before this smoke hook can execute inside the
  guest.
- After the appliance rebuild, a local-upstream HTTP smoke passed through the
  Rust stream gateway. The guest requested `http://198.51.100.10/`, while
  `--local-http-smoke-upstream 198.51.100.10:80` mapped that destination to a
  frontend-owned loopback HTTP server. `guest-http-smoke.log` shows
  `HTTP/1.1 200 OK`, and `vmnet-events.log` contains `tcp_connected`,
  `http_request`, `guest_payload`, and `upstream_payload` for
  `198.51.100.10:80`.

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
   - A blocked destination, for example `169.254.169.254`, fails closed.

## Remaining Gap

`sandbox-wrap` still constructs the current QEMU command with `-netdev user`
and `hostfwd`, so running it as-is would validate the old path rather than this
Rust vmnet gateway. Use `agentvm-frontend launch` for the stream path.

The remaining launcher hardening gap is process lifecycle: the new Rust
launcher starts the embedded tasks and waits for QEMU, but it does not yet
implement clean signal propagation/shutdown for long unattended runs. Run the
first VM smoke interactively, inspect `.sandbox/docker-vm/run/console.log` and
`.sandbox/docker-vm/run/qemu.log`, then append the outcome here.

`wra-p7m4` should remain open until that real VM smoke is run and the observed
outcome is appended here or to the ticket notes.
