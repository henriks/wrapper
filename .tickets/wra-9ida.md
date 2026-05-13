---
id: wra-9ida
status: open
deps: [wra-z9sh, wra-nrz2]
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
