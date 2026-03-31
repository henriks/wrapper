---
id: wra-ee8x
status: closed
deps: [wra-iyae, wra-nsc9]
links: []
created: 2026-03-27T21:22:04Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-hggq
tags: [docker, vm, cloud-hypervisor]
---
# Implement project-local Cloud Hypervisor lifecycle management

Add host-side code that creates, boots, supervises, and tears down the project-specific VM for one sandbox run.

## Design

Scope:
- create runtime directories under .sandbox/docker-vm/
- create and maintain the sparse Docker data disk in that directory
- start virtiofsd
- create the VM through the Cloud Hypervisor API
- tie VM lifetime to the wrapper process so sandbox exit triggers VM shutdown
- stop and clean up transient runtime resources on sandbox exit or failed startup

The launcher should be idempotent within one sandbox launch and leave the project recoverable after failures.

## Acceptance Criteria

Starting the VM is idempotent within one sandbox launch.

Sandbox termination shuts down the VM and removes transient runtime resources.

Failed startup leaves the project recoverable without manual host cleanup.


## Notes

**2026-03-27T21:26:46Z**

Runtime contract decision from wra-iyae: the wrapper must supervise the VM for the full sandbox lifetime instead of spawning a detached background daemon. Hold the project lock until sandbox exit, clean up .sandbox/docker-vm/run/ on clean exit, and preserve failed-run logs/state until the next start or reset.

**2026-03-27T21:31:58Z**

Artifact contract from wra-nsc9: consume docker/out/artifact-manifest.json instead of hardcoding appliance paths. Current manifest shape includes kernel, initrd, rootfs, root disk device /dev/vda, docker data device /dev/vdb, virtio-fs tag, vsock port, and the required kernel cmdline init=/usr/local/sbin/agentvm-init.

**2026-03-27T21:50:38Z**

Appliance build input changed under wra-nsc9: artifact manifest shape is still stable, but version metadata now reports Alpine package versions instead of Debian/Docker apt package versions. Launcher should keep consuming artifact-manifest.json and avoid assumptions about guest distro.

**2026-03-27T21:59:07Z**

Implemented the host-side VM lifecycle manager inside sandbox-wrap. Added DockerVmManager with project-local runtime paths under .sandbox/docker-vm/, manifest loading from docker/out/artifact-manifest.json, host prerequisite checks, flock-based project locking, sparse ext4 docker-data.raw creation plus metadata, virtiofsd/cloud-hypervisor process launch, Cloud Hypervisor REST API create/boot/shutdown calls over the Unix API socket, state.json updates, and lock-aware --reset protection. Verification completed: python compilation, CLI help, and lock behavior. Verification gap: full VM boot/shutdown was not exercised here because cloud-hypervisor and virtiofsd are not installed in this environment.

**2026-03-27T22:30:27Z**

Host-specific compatibility fix after initial implementation: Cloud Hypervisor on this host (v25.0.0) is killed by SIGSYS on the first API request with seccomp enabled. Updated sandbox-wrap to launch Cloud Hypervisor with --seccomp false by default, overrideable through SANDBOX_WRAP_CLOUD_HYPERVISOR_SECCOMP if needed.

**2026-03-28T09:16:00Z**

Readiness probe follow-up from live host testing: Cloud Hypervisor's `/api/v1/vmm.ping` replies with `Connection: keep-alive` and a `Content-Length` body. The original launcher client waited for EOF, which could misreport a healthy API socket as a timeout. Updated sandbox-wrap to parse headers and read the declared body length instead, and to include the last probe error in API/Docker readiness timeout messages for easier host-side diagnosis.

**2026-03-28T09:23:00Z**

Additional host interoperability fix from live testing: Cloud Hypervisor's Unix API socket on this host resets the connection when the client half-closes its write side after sending the request. Removed `shutdown(SHUT_WR)` from the launcher's Cloud Hypervisor and Docker Unix-socket HTTP probes. A direct Python probe using the wrapper's updated reader now returns `200` from `/api/v1/vmm.ping` without resets.

**2026-03-28T09:31:00Z**

Cloud Hypervisor API schema follow-up from live host testing: the host is running Cloud Hypervisor v25.0.0, whose `vm.create` schema still expects top-level `kernel`, `initramfs`, and `cmdline` members. The launcher had switched to the newer nested `payload` shape, which caused `vm.boot` to fail with `ConfigValidation(KernelMissing)`. Updated `build_vm_config()` back to the v25-compatible top-level fields. Official API example and release notes indicate the top-level fields remain accepted on newer releases as a compatibility path.

**2026-03-28T10:02:00Z**

Live host testing reached a remaining virtio-fs interoperability issue on Arch packaging: `cloud-hypervisor v25.0.0` gets through `vm.create` and begins `vm.boot`, but fails at `CreateVirtioFs(VhostUserGetProtocolFeatures(VhostUserProtocol(InvalidMessage)))` when talking to the only available host backend, `/usr/lib/virtiofsd` from the Arch `virtiofsd 1.13.3-1` package. Wrapper-side launch fixes improved diagnostics (API keep-alive handling, no Unix-socket half-close, verbose Cloud Hypervisor logging, simpler virtiofsd flags), but did not resolve the protocol mismatch. Current evidence suggests the next fix is host-package alignment rather than more wrapper logic: either a Cloud Hypervisor build/version paired with a compatible virtiofsd backend, or an alternate backend binary such as QEMU's virtiofsd if available on the target host.

**2026-03-31T20:10:00Z**

Live host debugging against `cloud-hypervisor 51.1` uncovered a host-proxy protocol bug after the guest successfully booted and `dockerd` started. The wrapper connected to the Cloud Hypervisor vsock Unix socket, wrote `CONNECT <port>\n`, and immediately began proxying Docker HTTP traffic. Cloud Hypervisor's current host-side vsock protocol requires waiting for an `OK <local_port>\n` acknowledgment before treating the connection as established. The project-local Docker proxy in `sandbox-wrap` was updated to read and validate that ack before forwarding traffic. This explains the earlier symptom where `proxy.log` showed repeated "guest vsock connection established" messages while the guest-side bridge never logged an accepted client.

**2026-03-31T20:22:00Z**

Live use after the Docker VM path started working exposed an interaction bug with terminal input: Cloud Hypervisor's default console mode is `tty`, and because the launcher did not override it, the guest console attached directly to the host terminal and consumed escape sequences and keypresses intended for the agent UI. Updated `build_vm_config()` to set both `serial` and `console` to `Null` explicitly so the guest console no longer binds to the user's TTY during normal `--docker` runs.

**2026-03-31T20:28:00Z**

Live use also showed the wrapper could linger noticeably after the agent process exited because the shutdown path allowed up to 10 seconds for graceful VM teardown before terminating helper processes. Updated `shutdown(clean=True)` to use a fast-path timeout of 1 second on normal sandbox exit and to terminate the host proxy and virtiofsd immediately while the VM shutdown request is in flight. This preserves the slower path for failed-start diagnostics but makes normal agent exit much more responsive.
