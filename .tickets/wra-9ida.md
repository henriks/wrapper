---
id: wra-9ida
status: closed
deps: [wra-nrz2]
links: []
created: 2026-05-13T10:23:17Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, qemu, network, integration]
---
# Integrate Rust vmnet gateway into QEMU frontend launch

Wire the Rust userspace gateway into the Rust frontend QEMU launch path. QEMU should use virtio-net-device with -netdev stream over a project-local Unix socket owned by the frontend, replacing QEMU user-mode networking for the Rust path.

## Design

Use the feasibility spike command shape and the frontend supervisor abstractions. Start the gateway before QEMU, wait for its Unix socket, pass the stream netdev args, supervise gateway/QEMU lifecycle together, and write logs/state under .sandbox/docker-vm/run. Preserve --no-net behavior by running the gateway in deny-all mode or by omitting outbound policy while retaining required host control if applicable.

## Acceptance Criteria

A Rust frontend launch can boot the microvm with stream networking and the guest obtains network config from the Rust gateway. State/logs show gateway lifecycle and policy mode. Failure to start the gateway fails before QEMU with actionable diagnostics. The old QEMU user-network path is not duplicated in the Rust frontend target.


## Notes

**2026-05-13T10:36:04Z**

wra-nrz2 converted composed-fs into an embeddable Rust library while retaining the CLI wrapper. Integration should wire the frontend to ServeConfig/serve_vhost_user_fs and avoid reintroducing Python-supervised filesystem startup as the target path.

**2026-05-13T10:38:58Z**

wra-52t3 created vm-frontend/ with command-shape tests. Integration should evolve this crate into the launcher, using its FrontendConfig and SupervisorPlan rather than translating through sandbox-wrap internals.

**2026-05-13T15:35:58Z**

Reordered with wra-z9sh: basic Rust stream launch and outbound vmnet smoke should happen before full host-to-guest Docker/payload/published-port restoration. This ticket should start the embedded vmnet gateway, composed fs, config fs, and QEMU stream netdev without adding a QEMU usernet fallback; document the smoke result as the outcome.

**2026-05-13T15:42:32Z**

Implemented the first Rust launch-integration slice. vm-frontend now loads docker/out/artifact-manifest.json into FrontendConfig, writes composed-fs/config-fs manifests from Rust, exposes agentvm-frontend prepare and launch commands, and starts embedded composed-fs/config-fs/vmnet tasks before spawning QEMU with -netdev stream. The prepare command was run against real local artifacts and printed a stream-only QEMU command with no usernet/hostfwd. Remaining work: interactive VM smoke and launcher lifecycle hardening for signal propagation/clean shutdown.

**2026-05-13T15:43:32Z**

Fixed two launch-path mismatches found during prepare validation: RuntimePaths now points docker-data.raw at .sandbox/docker-vm/docker-data.raw instead of run/docker-data.raw, and the config export socket uses guest-config.sock. Artifact kernel cmdline is normalized from console=hvc0 to console=ttyS0,115200n8 for the microvm isa-serial device. Re-ran vm-frontend tests and real prepare; both passed and the generated command now references the existing data disk path.

**2026-05-13T15:54:10Z**

Ran a bounded 10s agentvm-frontend launch outside the sandbox so QEMU could access /dev/kvm. Embedded composed-fs and config-fs started, QEMU connected with only vhost-user feature warnings, and console.log reached guest init lines: loaded virtio_net, configured eth0 as 10.0.2.15/24 via 10.0.2.2, started dockerd/socket bridge/payload server. QEMU was then killed by the intentional --qemu-timeout-seconds guard. This validates basic stream launch/boot wiring, but not outbound HTTP or clean lifecycle state yet.

**2026-05-13T15:55:32Z**

Added state.json lifecycle output to the Rust launcher. It now writes starting/running/exited states with machine_type=microvm, network_backend=stream, composed_fs=embedded, qemu pid/status, and socket paths. Re-ran a bounded 5s KVM launch; state.json ended at exited with qemu_status 'signal: 9 (SIGKILL)' from the intentional timeout, and no qemu process remained.

**2026-05-13T15:56:58Z**

State.json now includes policy mode fields. A 3s bounded launch with --allow-public-internet ended with egress_default_action=AllowPublicInternet and egress_reason=ExplicitAllowProfile in .sandbox/docker-vm/run/state.json.
