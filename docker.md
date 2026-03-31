## Spec: immutable Docker appliance VM using Cloud Hypervisor

Note: this file is now historical design context. The implemented build has
since pivoted from Debian to a smaller Alpine-based appliance. For the current
build and runtime behavior, see `docker/README.md`, `docker/OPERATIONS.md`, and
`docker/runtime-contract.md`.

### 1) Purpose

This design provides a **host-visible Docker socket** for an agent running on Linux, while keeping the Docker daemon itself **inside a lightweight VM**. The VM is treated as an **immutable, throwaway appliance**: no SSH, no interactive administration, no in-place upgrades, and no persistent mutable state except the Docker data disk and any explicit workspace share. Cloud Hypervisor is a good fit because it is a KVM-based VMM aimed at modern cloud workloads, supports direct kernel boot, a local API socket, `virtio-fs`, and `vsock`. ([GitHub][1])

### 2) Goals

The system should:

* expose a **Unix Docker socket on the host** for the agent to use;
* keep **`dockerd` private inside the guest** on `/var/run/docker.sock`;
* use a **read-only root image** for the guest OS;
* store Docker state on a **separate sparse raw disk image**;
* share agent workspaces with the guest using **`virtio-fs`**;
* create, boot, stop, and destroy VMs through the **Cloud Hypervisor API**. Docker listens on a Unix socket by default on Linux, and Cloud Hypervisor exposes `/vm.create`, `/vm.boot`, and `/vm.shutdown` through its API socket. ([Docker Documentation][2])

### 3) Non-goals

This VM is **not** a general-purpose server. It does not need SSH, package management at runtime, login users, or long-lived mutable root state. It is rebuilt as an image artifact when the kernel, Docker version, or bridge binary changes. That model also avoids exposing Docker over TCP, which Docker documents as something that must be secured carefully because the daemon normally listens on a local Unix socket. ([Docker Documentation][2])

### 4) High-level architecture

At runtime, each sandbox instance has five pieces:

1. **Host agent** inside `bubblewrap`, with only a scoped Unix socket bind-mounted in.
2. **Host Unix-socket proxy** at `/run/agent-vms/<id>/docker.sock`.
3. **Cloud Hypervisor VM** with one read-only root disk, one sparse Docker-data disk, one `virtio-fs` share, and one `vsock` device.
4. **Guest Unix-socket bridge** that forwards a `vsock` port to `/var/run/docker.sock`.
5. **Guest Docker daemon** plus `containerd`.

Cloud Hypervisor `vsock` is stream-based, and its host-to-guest connection model uses the VM’s Unix socket with a `CONNECT <port>` preamble, so a tiny host-side proxy is the clean way to present a normal Unix socket to the agent. ([GitHub][3])

### 5) Host requirements

The host must be Linux with **KVM** available. Cloud Hypervisor’s quick-start documentation says the minimum host kernel version for required KVM functionality is **4.11**, the minimum recommended version for adequate performance is **5.6**, and their CI mostly runs on **5.15**. ([Cloud Hypervisor][4])

The host also needs:

* **Cloud Hypervisor v51.0 or newer**;
* **`virtiofsd`** for the workspace share;
* host kernel support for **`vhost-vsock`** if `vsock` is used.

Cloud Hypervisor’s `virtio-fs` documentation says the host-side backend is a dedicated `virtiofsd` process, and its `vsock` documentation requires host support for `CONFIG_VHOST_VSOCK`. Cloud Hypervisor v51.0 fixed a disk-image handling vulnerability and added explicit image-type selection plus `backing_files=on|off`; that release also documents `sparse=on` as the default for thin-provisioned raw disks with reclaim support. ([GitHub][5])

### 6) Guest requirements

The guest should be **as small as possible while still using a supported Docker installation path**. The recommended guest base is **Debian 12 minimal** built as an appliance rootfs. Docker’s Debian install docs currently support Debian 11, 12, and 13, and support installing a **specific pinned version** from Docker’s apt repository. Docker’s convenience script is documented as testing/development-oriented, and Docker’s standalone binaries are documented as not recommended for production because they do not receive automatic security updates from the distro. ([Docker Documentation][6])

The guest kernel must support:

* `virtio-blk`
* `virtio-fs`
* `virtio-vsock`
* `ext4`
* `overlayfs`
* normal container primitives required by Docker

Cloud Hypervisor supports direct kernel boot on x86-64 with either a **PVH-capable kernel** or a **regular `bzImage`**, and its `virtio-fs` docs say modern Linux guests should be **at least 5.10** for supported `virtio-fs` use. Docker’s `overlay2` docs require a Linux kernel **4.0+** or equivalent distro backports. ([GitHub][1])

### 7) Guest filesystem layout

Inside the guest, the layout is:

* `/` → immutable root image, mounted read-only
* `/var/lib/docker` → separate sparse raw disk
* `/workspace` → `virtio-fs` mount from the host
* `/run` and `/tmp` → tmpfs

`/var/lib/docker` must **not** live on the shared workspace mount. Docker’s storage-driver docs say the backing filesystem for Docker’s data directory matters, and `overlay2` is supported on `ext4` and on `xfs` when `d_type=true` / `ftype=1`. This is why Docker state belongs on a dedicated block-backed filesystem, not on the `virtio-fs` share. ([Docker Documentation][7])

The recommended filesystem for the Docker data disk is **ext4**. Use XFS only if you have a specific reason to prefer it and can guarantee the required `ftype=1` setting. ([Docker Documentation][8])

### 8) Runtime process model

The guest is an appliance and should boot into a **tiny PID 1** that does only this:

1. mount `/var/lib/docker`;
2. mount `/workspace` with `virtio-fs`;
3. start `containerd`;
4. start `dockerd`;
5. start the guest-side `vsock` bridge.

Docker’s docs explicitly describe starting `dockerd` manually, and show it listening on `unix:///var/run/docker.sock` by default when started in the foreground. That makes a tiny custom init perfectly viable; `systemd` is optional, not required. ([Docker Documentation][9])

### 9) Host-visible Docker socket model

Inside the guest, `dockerd` listens only on:

```text
/var/run/docker.sock
```

The guest bridge listens on a chosen `vsock` port, for example `1075`, and forwards connections to `/var/run/docker.sock`. The host proxy listens on:

```text
/run/agent-vms/<id>/docker.sock
```

and forwards each connection into the VM over `vsock`.

The host agent then uses:

```text
DOCKER_HOST=unix:///run/agent-vms/<id>/docker.sock
```

This keeps Docker off TCP entirely, which matches Docker’s documented default and avoids exposing the daemon over a network listener. ([Docker Documentation][2])

### 10) Cloud Hypervisor device model

Each VM should be configured with:

* **2 vCPUs**
* **2 GiB RAM**
* **1 read-only root disk**
* **1 sparse raw Docker data disk**
* **1 `virtio-fs` share**
* **1 `vsock` device**
* **1 virtio-net NIC** with outbound access

Cloud Hypervisor’s API supports creating and booting VMs over its Unix API socket, and its `virtio-fs` docs require `--memory shared=on` when using `--fs`. The guest mounts the share with `mount -t virtiofs <tag> <mountpoint>`. ([GitHub][10])

A representative configuration shape is:

```json
{
  "cpus": { "boot_vcpus": 2, "max_vcpus": 2 },
  "memory": { "size": 2147483648, "shared": true },
  "payload": {
    "kernel": "/opt/docker-appliance/vmlinuz",
    "cmdline": "console=hvc0 root=/dev/vda ro quiet"
  },
  "disks": [
    {
      "path": "/opt/docker-appliance/rootfs.raw",
      "readonly": true,
      "image_type": "raw",
      "backing_files": false
    },
    {
      "path": "/var/lib/agent-vms/<id>/docker-data.raw",
      "readonly": false,
      "image_type": "raw",
      "backing_files": false,
      "sparse": true
    }
  ],
  "fs": [
    {
      "tag": "workspace",
      "socket": "/run/agent-vms/<id>/virtiofs.sock"
    }
  ],
  "vsock": {
    "cid": 1000,
    "socket": "/run/agent-vms/<id>/ch.vsock"
  }
}
```

The important part is to **set `image_type=raw` explicitly** and **`backing_files=false`** on both disks. Cloud Hypervisor v51.0 added explicit image-type selection and made backing files controllable with a secure default after the raw-image handling vulnerability disclosed in GHSA-jmr4-g2hv-mjj6. ([GitHub][11])

### 11) Docker data disk behavior

The Docker data disk is a **sparse raw image** on the host, mounted inside the guest at `/var/lib/docker`. Cloud Hypervisor’s v51.0 release notes say `virtio-blk` supports `DISCARD` and `WRITE_ZEROES` for raw images, and `sparse=on` enables thin provisioning with space reclamation when the guest trims the filesystem. ([GitHub][11])

That means the host file can grow as data is written. If you later need a larger logical disk size, resize the virtual disk and then grow the filesystem inside the guest. Cloud Hypervisor’s API includes VM management primitives and, per release notes, live disk resize support for raw images. ([GitHub][10])

### 12) Building the guest image

#### Recommended build path

Use **`debootstrap`** to create a minimal Debian rootfs in CI, customize it in a chroot or container, then pack it into a raw ext4 image. Debian’s documentation describes `debootstrap` as a tool that installs Debian base systems into a subdirectory of an already installed system, which is exactly the right primitive for building a tiny appliance rootfs. ([Debian Wiki][12])

Why this is the recommended path:

* it produces the smallest, most controlled rootfs;
* it uses Docker’s **supported apt-repository install path**;
* it keeps runtime immutable while still getting distro-managed packages at build time. Docker’s Debian docs show the official package set and support installing specific versions from the Docker apt repository. ([Docker Documentation][6])

#### Alternative build path

If you want a more structured image factory, **`mkosi`** is a good alternative. Its documentation says it can emit a **raw GPT disk image**, a plain directory tree, tar, cpio, and other formats. ([GitHub][13])

For this specific appliance, though, `debootstrap` is the better default because it is simpler and gives tighter control over exactly what lands in the guest.

### 13) Image build procedure

The build pipeline should do this:

1. create Debian rootfs with `debootstrap --variant=minbase`;
2. chroot into it;
3. add Docker’s official apt repository;
4. install pinned versions of:

   * `docker-ce`
   * `docker-ce-cli`
   * `containerd.io`
   * `docker-buildx-plugin`
   * `docker-compose-plugin`
5. add the tiny PID 1 binary or script;
6. add the guest `vsock` bridge binary;
7. add mountpoint directories for `/var/lib/docker` and `/workspace`;
8. remove apt caches, docs, locales, and anything not needed at runtime;
9. pack the rootfs into a raw ext4 image and boot it read-only.

Docker’s Debian docs list those packages and show installing a pinned `docker-ce` version from the apt repository. They also note that the convenience script is for testing/development, not production. ([Docker Documentation][6])

### 14) Boot strategy

Start with a **Debian kernel + initrd** to get the first version working quickly. Cloud Hypervisor supports direct kernel boot and also supports booting with a kernel and initrd. Once the appliance is stable, replace that with a **custom `bzImage`** that has the needed virtio and container features built in, so the VM can boot with fewer moving parts. Cloud Hypervisor’s documented direct-boot support on x86-64 includes both PVH-capable kernels and regular `bzImage` kernels. ([GitHub][1])

### 15) Minimal guest contents

The root image should contain only:

* kernel modules needed for the chosen kernel strategy;
* Docker Engine packages and `containerd`;
* the tiny init;
* the guest `vsock` bridge;
* mount utilities and minimal networking tools;
* CA certificates if the guest will pull from registries over TLS.

Do **not** include:

* SSH server
* compilers
* shells beyond what the init path strictly needs
* package-manager use at runtime
* editors or debug tooling in production

The point is to keep the root image immutable and disposable, while Docker state and workspaces live outside it. Docker’s package-install path is supported; Docker’s binary-install path is not recommended for production. ([Docker Documentation][6])

### 16) Security requirements

Use these defaults:

* Cloud Hypervisor **v51.0+**
* explicit `image_type=raw`
* `backing_files=false`
* read-only root image
* no guest SSH
* no Docker TCP listener
* host agent sees only the per-VM Unix socket
* destroy and recreate VMs rather than modifying them in place

The disk-image handling advisory and v51.0 release notes make the first three settings especially important. Docker’s remote-access docs make the “no TCP listener” choice the safer default. ([GitHub][14])

### 17) Default implementation choice

**Recommended implementation**

* **VMM:** Cloud Hypervisor
* **guest distro:** Debian 12 minimal
* **build tool:** `debootstrap`
* **root image:** raw ext4, read-only
* **Docker data disk:** raw ext4, sparse
* **workspace sharing:** `virtio-fs`
* **Docker access:** host Unix socket proxy → guest `vsock` bridge → guest `/var/run/docker.sock`
* **init model:** tiny custom PID 1, no systemd required

This is the best fit for an immutable “Docker appliance VM” because it uses supported Docker packaging, Cloud Hypervisor’s native `virtio-fs`/`vsock`/API model, and keeps the operational surface as small as possible. ([Docker Documentation][6])

If you want, I can turn this into an implementation packet next: build commands, a sample `debootstrap` script, minimal PID 1, and the two tiny socket-bridge processes.

[1]: https://github.com/cloud-hypervisor/cloud-hypervisor?utm_source=chatgpt.com "cloud-hypervisor/cloud-hypervisor: A Virtual Machine ..."
[2]: https://docs.docker.com/engine/daemon/remote-access/ "Configure remote access for Docker daemon | Docker Docs"
[3]: https://github.com/cloud-hypervisor/cloud-hypervisor/blob/main/docs/vsock.md "cloud-hypervisor/docs/vsock.md at main · cloud-hypervisor/cloud-hypervisor · GitHub"
[4]: https://www.cloudhypervisor.org/docs/prologue/quick-start/ "Quick Start - Cloud Hypervisor"
[5]: https://github.com/cloud-hypervisor/cloud-hypervisor/blob/main/docs/fs.md "cloud-hypervisor/docs/fs.md at main · cloud-hypervisor/cloud-hypervisor · GitHub"
[6]: https://docs.docker.com/engine/install/debian/ "Debian | Docker Docs"
[7]: https://docs.docker.com/engine/storage/drivers/select-storage-driver/?utm_source=chatgpt.com "Select a storage driver"
[8]: https://docs.docker.com/engine/storage/drivers/overlayfs-driver/ "OverlayFS storage driver | Docker Docs"
[9]: https://docs.docker.com/engine/daemon/start/ "Start the daemon | Docker Docs"
[10]: https://github.com/cloud-hypervisor/cloud-hypervisor/blob/main/docs/api.md "cloud-hypervisor/docs/api.md at main · cloud-hypervisor/cloud-hypervisor · GitHub"
[11]: https://github.com/cloud-hypervisor/cloud-hypervisor/blob/main/release-notes.md "cloud-hypervisor/release-notes.md at main · cloud-hypervisor/cloud-hypervisor · GitHub"
[12]: https://wiki.debian.org/Debootstrap?utm_source=chatgpt.com "Debootstrap - Debian Wiki"
[13]: https://github.com/systemd/mkosi/blob/main/mkosi/resources/man/mkosi.1.md "mkosi/mkosi/resources/man/mkosi.1.md at main · systemd/mkosi · GitHub"
[14]: https://github.com/cloud-hypervisor/cloud-hypervisor/security/advisories/GHSA-jmr4-g2hv-mjj6 "Host File Exfiltration via QCOW Backing File Abuse · Advisory · cloud-hypervisor/cloud-hypervisor · GitHub"
