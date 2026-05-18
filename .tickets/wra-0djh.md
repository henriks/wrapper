---
id: wra-0djh
status: closed
deps: []
links: [wra-hizo, wra-b9b1, wra-p06p, wra-d02u]
created: 2026-05-18T12:32:55Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-emj5
tags: [cleanup, guest, appliance, boot]
---
# Audit guest dmesg for unnecessary boot modules and services

Run a live guest boot and inspect dmesg plus guest init logs to identify kernel modules, drivers, filesystems, services, or package dependencies that are loaded but unnecessary for AgentVM's current microvm path. The goal is to trim appliance boot surface and avoid carrying historical dependencies. This should include checking whether warnings/noisy probes point to disabled features such as IPv6, unused devices, obsolete q35 assumptions, or packages like bubblewrap that are no longer used.

## Design

Capture dmesg from a normal live-smoke or dedicated diagnostic payload early enough after boot to see module/device probes. Compare loaded modules and services against the required runtime set: virtio block, virtio net, virtiofs/config fs, overlay/root state, Docker, Rust guest service, socket bridge if still present, and required setup-tool dependencies. Document each candidate as keep/remove/needs proof. Create follow-up tickets for any trim that changes appliance package pins, kernel config, guest-init modprobe calls, or validation assumptions.

## Acceptance Criteria

A dmesg/module/service inventory is recorded in ticket notes or docs; each unnecessary or suspicious item has a clear action, follow-up ticket, or explicit retain rationale; at least one normal live boot path is used for evidence; no package/module is removed without the relevant live validation tier; required validation evidence or live-environment limitation is recorded before close.


## Notes

**2026-05-18T12:45:04Z**

Live guest boot audit evidence captured after Henrik's fresh appliance rebuild. Commands/artifacts:
- `cargo run --manifest-path vm-frontend/Cargo.toml --offline --bin agentvm -- --project "$PWD" --no-tui --artifact-manifest "$PWD/docker/out/artifact-manifest.json" --qemu /usr/bin/qemu-system-x86_64 -- /bin/sh -lc '...'` wrote `.sandbox/audit/wra-0djh/live-audit-payload.txt`.
- A second normal live boot used privileged Docker inside the guest to read kernel dmesg (`docker run --rm --privileged --pid=host alpine:3.22 dmesg`) and wrote `.sandbox/audit/wra-0djh/live-dmesg-via-privileged-container.txt`; direct non-root payload `dmesg` is denied, as expected.
- Mirrored logs copied to `.sandbox/audit/wra-0djh/live-console.log`, `guest-dockerd.log`, `guest-docker-bridge.log`, and `guest-payload-server.log`.

Inventory / disposition:
- Required runtime modules present and retained: virtio_blk for root/state disks, virtiofs+fuse for project/config shares, virtio_net for vmnet, ext4+jbd2/mbcache/crc modules for root/state, overlay for the root/state overlay, bridge/stp/llc/nf_tables/nft_chain_nat/nf_nat/nf_conntrack/x_tables/ip_set/xt_* for Docker networking/NAT, and virtio_rng/rng_core.
- IPv6: guest init now logs `disabled IPv6 for all/default/eth0`; `ip addr` shows no inet6 addresses and `/proc/sys/net/ipv6/conf/{all,default}/disable_ipv6` are `1`. Kernel still registers PF_INET6 and Docker conntrack loads `nf_defrag_ipv6`; retain as Docker/netfilter side effects while vmnet IPv6 deny/log remains defense in depth.
- Headless/no-hardware probes: dmesg includes expected microvm/Alpine-virt noise (`PCI: System does not support PCI`, ACPI disabled, i8042 no controller, rtc_cmos inaccessible, amd_pstate unavailable, SCSI/libata/VMware PVSCSI registrations). No package/module removal recommended in this cleanup pass because these come from the generic Alpine virt kernel rather than our appliance package list; trimming would require a separate kernel/config pass.
- Graphics/framebuffer: `simpledrm`, drm helpers, fb, and i2c_core are loaded despite serial-console/headless usage. Retain for now for the same generic-kernel reason; consider only if a future custom/minimized kernel pass is justified.
- Docker storage probe noise: dockerd/containerd works but logs avoidable unsupported storage/snapshotter probes (overlay2 invalid on overlay-backed `/var/lib/docker`, missing fuse-overlayfs, skipped btrfs/devmapper/erofs/zfs). Created linked follow-up `wra-d02u` to pin/quiet Docker storage configuration or document why the probe noise must remain.
- Services/processes retained: PID 1 `agentvm-init`, dockerd+containerd, Rust `agentvm-guest-service docker-bridge`, Rust payload server, and log mirror tails. No stale Python bridge/payload services and no bubblewrap process observed.

Validation: `./vm-frontend/validate.sh required` passed in iteration 25 after the fresh appliance rebuild. No code was changed for this audit ticket; no additional appliance rebuild is required by the audit itself.
