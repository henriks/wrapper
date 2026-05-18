# Microvm Boot and Device Feasibility Spike

Ticket: `wra-mjer`

Date: 2026-05-12

## Outcome

The current appliance can boot under QEMU `microvm` with KVM when PCI devices
are replaced by virtio-mmio devices and ACPI is disabled.

Validated locally:
- QEMU version: `10.2.2`
- machine type: `microvm`
- kernel: `docker/out/vmlinuz`, Alpine `6.12.79-0-virt`
- rootfs: `docker/out/rootfs.raw`
- initrd: `docker/out/initrd.img`
- two upstream `virtiofsd` exports: workspace and config
- QEMU user-mode networking with `hostfwd`
- guest Docker bridge TCP path
- guest payload control TCP path

The working command shape uses:
- `-machine microvm,acpi=off,memory-backend=mem,isa-serial=on`
- `-enable-kvm -cpu host`
- `virtio-blk-device` instead of `virtio-blk-pci`
- `virtio-net-device` instead of `virtio-net-pci`
- `virtio-rng-device` instead of `virtio-rng-pci`
- `vhost-user-fs-device` instead of `vhost-user-fs-pci`
- `-netdev user,...,hostfwd=...` plus explicit `virtio-net-device`

This is enough evidence to implement the later `microvm` command branch after
`q35 + composed fs` is validated.

## Important Discovery: Disable ACPI

The first KVM test used:

```sh
-machine microvm,memory-backend=mem,isa-serial=on
```

That boot reached the kernel but did not show injected `virtio_mmio.device=`
arguments in `/proc/cmdline`. The guest therefore could not discover the
non-PCI virtio devices.

The successful run used:

```sh
-machine microvm,acpi=off,memory-backend=mem,isa-serial=on
```

With `acpi=off`, QEMU appended six `virtio_mmio.device=` arguments:

```text
virtio_mmio.device=512@0xfeb00e00:12
virtio_mmio.device=512@0xfeb00c00:11
virtio_mmio.device=512@0xfeb00a00:10
virtio_mmio.device=512@0xfeb00800:9
virtio_mmio.device=512@0xfeb00600:8
virtio_mmio.device=512@0xfeb00400:7
```

The guest then registered six virtio-mmio devices and booted successfully.

## Kernel Support

The current Alpine virt kernel has the required support:

```text
CONFIG_VIRTIO=y
CONFIG_VIRTIO_MMIO=y
CONFIG_VIRTIO_MMIO_CMDLINE_DEVICES=y
CONFIG_VIRTIO_BLK=m
CONFIG_VIRTIO_NET=m
CONFIG_VIRTIO_FS=m
CONFIG_VIRTIO_CONSOLE=y
CONFIG_SERIAL_8250=y
CONFIG_SERIAL_8250_CONSOLE=y
```

The rootfs contains the expected modules:
- `virtio_blk.ko.gz`
- `virtio_net.ko.gz`
- `virtio-rng.ko.gz`
- `virtiofs.ko.gz`

## Device Budget

The successful compatibility run used six virtio-mmio devices:
- rootfs block
- Docker data block
- rng
- workspace virtio-fs
- config virtio-fs
- network

QEMU documentation states that `microvm` supports up to eight user-configured
virtio-mmio devices and does not support PCI-only devices. The target composed
filesystem design should fit comfortably:
- rootfs block
- Docker data block
- rng
- network
- composed filesystem
- optional config filesystem if still needed

That is five or six devices, within the limit.

Source: https://www.qemu.org/docs/master/system/i386/microvm.html

## Working Command Shape

This command shape booted the current guest and reached Docker and payload
readiness. Paths and ports below are representative.

```sh
qemu-system-x86_64 \
  -enable-kvm \
  -cpu host \
  -nodefaults \
  -no-user-config \
  -no-reboot \
  -display none \
  -serial file:/tmp/agentvm-microvm/console.log \
  -monitor none \
  -machine microvm,acpi=off,memory-backend=mem,isa-serial=on \
  -object memory-backend-memfd,id=mem,size=2147483648,share=on \
  -m 2048 \
  -smp 2 \
  -kernel /home/hsaksela/ai/wrapper/docker/out/vmlinuz \
  -initrd /home/hsaksela/ai/wrapper/docker/out/initrd.img \
  -append "console=ttyS0,115200n8 root=/dev/vda rootfstype=ext4 ro init=/usr/local/sbin/agentvm-init quiet" \
  -drive if=none,file=/home/hsaksela/ai/wrapper/docker/out/rootfs.raw,format=raw,readonly=on,id=rootfs \
  -device virtio-blk-device,drive=rootfs \
  -drive if=none,file=/tmp/agentvm-microvm/docker-data.raw,format=raw,id=dockerdata \
  -device virtio-blk-device,drive=dockerdata \
  -object rng-random,id=rng0,filename=/dev/urandom \
  -device virtio-rng-device,rng=rng0 \
  -chardev socket,id=charfs,path=/tmp/agentvm-microvm/workspace.sock \
  -device vhost-user-fs-device,chardev=charfs,tag=workspace,queue-size=128,num-request-queues=1 \
  -chardev socket,id=charcfg,path=/tmp/agentvm-microvm/config.sock \
  -device vhost-user-fs-device,chardev=charcfg,tag=agentvm-config,queue-size=128,num-request-queues=1 \
  -netdev user,id=net0,ipv6=off,hostname=agentvm,net=10.0.2.0/24,host=10.0.2.2,dns=10.0.2.3,restrict=off,hostfwd=tcp:127.0.0.1:39375-:1075,hostfwd=tcp:127.0.0.1:39376-:1076 \
  -device virtio-net-device,netdev=net0,mac=02:fc:12:34:56:78
```

## Observed Guest Boot Evidence

The successful boot log included:

```text
Kernel command line: ... virtio_mmio.device=512@0xfeb00e00:12 ...
virtio-mmio: Registering device virtio-mmio.0 ...
virtio-mmio: Registering device virtio-mmio.5 ...
virtio_blk virtio0: [vda] ...
virtio_blk virtio1: [vdb] ...
Mounting root: ok.
EXT4-fs (vdb): mounted filesystem ... r/w
agentvm-init: loaded kernel module virtio_net
agentvm-init: configured network on eth0 (10.0.2.15/24 via 10.0.2.2)
agentvm-init: starting dockerd
agentvm-init: starting socket bridge
agentvm-init: starting payload server
```

Both upstream `virtiofsd` processes logged client connections and guest FUSE
traffic. The config share saw `Init`, `Lookup`, `Open`, `Read`, `Poll`
returning `ENOSYS`, and `Release`; this confirms guest init mounted and read
the config share.

## Hostfwd Validation

Payload control probe against the forwarded host port returned the expected
framed response:

```text
b'K\x00\x00\x00\x02'
b'ok'
```

Docker `_ping` against the forwarded host Docker port returned:

```text
HTTP/1.1 200 OK
Server: Docker/28.3.3 (linux)

OK
```

This confirms QEMU user-mode networking and `hostfwd` survive the `microvm`
device model.

## Implementation Guidance

For `wra-ek03`, implement the `microvm` branch using the command shape above
after `q35 + composed fs` validation.

Required changes from the current q35 branch:
- replace `-machine q35,accel=kvm,memory-backend=mem` with
  `-machine microvm,acpi=off,memory-backend=mem,isa-serial=on`
- keep `-enable-kvm`; add `-cpu host` unless testing shows this is not needed
- replace PCI virtio devices with non-PCI `*-device` forms
- replace `-nic ...,model=virtio-net-pci` with `-netdev user,...` and
  `-device virtio-net-device,netdev=net0,mac=...`
- keep `memory-backend-memfd share=on` for vhost-user-fs
- keep direct kernel/initrd boot; do not attempt firmware block boot
- keep `console=ttyS0,115200n8` when `isa-serial=on`
- consider adding `reboot=t` to make guest-triggered shutdown faster under
  `-no-reboot`

## Follow-Up Notes

- The composed filesystem target remains necessary. This spike only proves the
  machine/device path with ordinary upstream `virtiofsd`.
- The current separate config filesystem is within the device budget, but the
  composed filesystem design should still absorb it if safe. If not, keeping a
  tiny config fs is acceptable from the `microvm` device-budget perspective.
- The sandbox blocks local TCP sockets and vhost-user socket creation; future
  VM integration tests may need host-namespace execution privileges.
