#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
source "${SCRIPT_DIR}/appliance.env"

readonly BUILD_DIR="${SCRIPT_DIR}/build"
readonly ROOTFS_DIR="${BUILD_DIR}/rootfs"
readonly OUT_DIR="${SCRIPT_DIR}/out"
readonly ROOTFS_IMAGE="${OUT_DIR}/rootfs.raw"
readonly MANIFEST_PATH="${OUT_DIR}/artifact-manifest.json"
readonly GUEST_INIT_PATH="${ROOTFS_DIR}/usr/local/sbin/agentvm-init"
readonly GUEST_BRIDGE_PATH="${ROOTFS_DIR}/usr/local/libexec/agentvm-socket-bridge"
readonly GUEST_PAYLOAD_SERVER_PATH="${ROOTFS_DIR}/usr/local/libexec/agentvm-payload-server"
readonly MINIROOTFS_TARBALL="${BUILD_DIR}/alpine-minirootfs.tar.gz"

require_root() {
  if [[ ${EUID} -ne 0 ]]; then
    echo "error: docker/build-appliance.sh must run as root" >&2
    exit 1
  fi
}

require_commands() {
  local missing=()
  local cmd
  for cmd in chroot curl tar truncate mkfs.ext4 sha256sum; do
    if ! command -v "${cmd}" >/dev/null 2>&1; then
      missing+=("${cmd}")
    fi
  done
  if ((${#missing[@]} > 0)); then
    printf 'error: missing required commands: %s\n' "${missing[*]}" >&2
    exit 1
  fi
}

require_version_pins() {
  local required=(
    DOCKER_TCP_PORT
    PAYLOAD_TCP_PORT
    CONFIG_VIRTIOFS_TAG
    DOCKER_ENGINE_VERSION
    DOCKER_CLI_VERSION
    LINUX_VIRT_VERSION
    MKINITFS_VERSION
    PYTHON3_VERSION
    E2FSPROGS_VERSION
    IPROUTE2_VERSION
    UTIL_LINUX_VERSION
    BUBBLEWRAP_VERSION
    NODEJS_VERSION
    NPM_VERSION
    MISE_VERSION
    MISE_TARGET
    MISE_SHA256
  )
  local name
  for name in "${required[@]}"; do
    if [[ -z "${!name:-}" ]]; then
      echo "error: ${name} must be set in docker/appliance.env" >&2
      exit 1
    fi
  done
}

minirootfs_url() {
  printf '%s/%s/releases/%s/alpine-minirootfs-%s-%s.tar.gz\n' \
    "${ALPINE_MIRROR}" \
    "${ALPINE_BRANCH}" \
    "${ALPINE_ARCH}" \
    "${ALPINE_VERSION}" \
    "${ALPINE_ARCH}"
}

clean_dirs() {
  rm -rf "${BUILD_DIR}"
  mkdir -p "${ROOTFS_DIR}" "${OUT_DIR}"
}

bootstrap_rootfs() {
  curl -fsSL "$(minirootfs_url)" -o "${MINIROOTFS_TARBALL}"
  tar -xzf "${MINIROOTFS_TARBALL}" -C "${ROOTFS_DIR}"
}

write_apk_repositories() {
  cat > "${ROOTFS_DIR}/etc/apk/repositories" <<EOF
${ALPINE_MIRROR}/${ALPINE_BRANCH}/main
${ALPINE_MIRROR}/${ALPINE_BRANCH}/community
EOF
}

install_guest_assets() {
  install -d \
    "${ROOTFS_DIR}/etc" \
    "${ROOTFS_DIR}/etc/mkinitfs" \
    "${ROOTFS_DIR}/usr/local/sbin" \
    "${ROOTFS_DIR}/usr/local/libexec"
  install -m 0755 "${SCRIPT_DIR}/guest-init.sh" "${GUEST_INIT_PATH}"
  install -m 0755 "${SCRIPT_DIR}/guest-socket-bridge.py" "${GUEST_BRIDGE_PATH}"
  install -m 0755 "${SCRIPT_DIR}/guest-payload-server.py" "${GUEST_PAYLOAD_SERVER_PATH}"
  cat > "${ROOTFS_DIR}/etc/agentvm.env" <<EOF
DOCKER_TCP_PORT=${DOCKER_TCP_PORT}
PAYLOAD_TCP_PORT=${PAYLOAD_TCP_PORT}
VIRTIOFS_TAG=${VIRTIOFS_TAG}
CONFIG_VIRTIOFS_TAG=${CONFIG_VIRTIOFS_TAG}
EOF
  cat > "${ROOTFS_DIR}/etc/mkinitfs/mkinitfs.conf" <<'EOF'
features="base virtio ext4"
EOF
  install -d \
    "${ROOTFS_DIR}/workspace" \
    "${ROOTFS_DIR}/var/lib/docker" \
    "${ROOTFS_DIR}/var/tmp" \
    "${ROOTFS_DIR}/run" \
    "${ROOTFS_DIR}/tmp" \
    "${ROOTFS_DIR}/var/log"
  chmod 1777 "${ROOTFS_DIR}/tmp" "${ROOTFS_DIR}/var/tmp"
}

run_in_chroot() {
  chroot "${ROOTFS_DIR}" /usr/bin/env -i \
    HOME=/root \
    PATH=/usr/sbin:/usr/bin:/sbin:/bin:/usr/local/sbin:/usr/local/bin \
    TERM="${TERM:-xterm}" \
    /bin/sh -euxc "$1"
}

install_mise_binary() {
  local asset_name="mise-${MISE_VERSION}-${MISE_TARGET}"
  local asset_url="https://github.com/jdx/mise/releases/download/${MISE_VERSION}/${asset_name}"
  local asset_path="${ROOTFS_DIR}/usr/local/bin/mise"

  curl -fsSL "${asset_url}" -o "${asset_path}"
  printf '%s  %s\n' "${MISE_SHA256}" "${asset_path}" | sha256sum -c -
  chmod 0755 "${asset_path}"
}

configure_rootfs() {
  cat > "${ROOTFS_DIR}/etc/hostname" <<'EOF'
agentvm
EOF

  cat > "${ROOTFS_DIR}/etc/hosts" <<'EOF'
127.0.0.1 localhost
127.0.1.1 agentvm
EOF

  cp /etc/resolv.conf "${ROOTFS_DIR}/etc/resolv.conf"

  run_in_chroot "apk update"
  run_in_chroot "apk add --no-cache docker-engine=${DOCKER_ENGINE_VERSION} docker-cli=${DOCKER_CLI_VERSION} linux-virt=${LINUX_VIRT_VERSION} mkinitfs=${MKINITFS_VERSION} python3=${PYTHON3_VERSION} e2fsprogs=${E2FSPROGS_VERSION} iproute2=${IPROUTE2_VERSION} util-linux=${UTIL_LINUX_VERSION} bubblewrap=${BUBBLEWRAP_VERSION} nodejs=${NODEJS_VERSION} npm=${NPM_VERSION} ca-certificates"
  run_in_chroot "kernel_version=\$(basename /lib/modules/*) && mkinitfs -b / \"\${kernel_version}\""
  run_in_chroot "rm -rf /var/cache/apk/* /usr/share/man/* /usr/share/doc/* /usr/share/locale/*"
  rm -f "${ROOTFS_DIR}/etc/resolv.conf"
  ln -s /run/resolv.conf "${ROOTFS_DIR}/etc/resolv.conf"
}

copy_kernel_artifacts() {
  local kernel_path
  local initramfs_path

  kernel_path=$(find "${ROOTFS_DIR}/boot" -maxdepth 1 -type f -name 'vmlinuz-*' | sort | tail -n 1)
  initramfs_path=$(find "${ROOTFS_DIR}/boot" -maxdepth 1 -type f -name 'initramfs-*' | sort | tail -n 1)

  if [[ -z "${kernel_path}" || -z "${initramfs_path}" ]]; then
    echo "error: failed to locate kernel or initramfs in ${ROOTFS_DIR}/boot" >&2
    exit 1
  fi

  cp "${kernel_path}" "${OUT_DIR}/vmlinuz"
  cp "${initramfs_path}" "${OUT_DIR}/initrd.img"
}

pack_rootfs() {
  rm -f "${ROOTFS_IMAGE}"
  truncate -s "${ROOTFS_SIZE}" "${ROOTFS_IMAGE}"
  mkfs.ext4 -F -d "${ROOTFS_DIR}" "${ROOTFS_IMAGE}"
}

write_manifest() {
  cat > "${MANIFEST_PATH}" <<EOF
{
  "schema_version": 1,
  "artifacts": {
    "kernel": "docker/out/vmlinuz",
    "initrd": "docker/out/initrd.img",
    "rootfs": "docker/out/rootfs.raw"
  },
  "vm": {
    "cpus": 2,
    "memory_bytes": 2147483648,
    "root_disk_device": "/dev/vda",
    "docker_data_device": "/dev/vdb",
    "virtiofs_tag": "${VIRTIOFS_TAG}",
    "kernel_cmdline": "console=hvc0 root=/dev/vda rootfstype=ext4 ro init=/usr/local/sbin/agentvm-init quiet"
  },
  "guest": {
    "docker_socket": "/var/run/docker.sock",
    "docker_tcp_port": ${DOCKER_TCP_PORT},
    "payload_tcp_port": ${PAYLOAD_TCP_PORT},
    "workspace_mount": "/workspace",
    "docker_data_mount": "/var/lib/docker"
  },
  "versions": {
    "alpine_version": "${ALPINE_VERSION}",
    "docker_engine": "${DOCKER_ENGINE_VERSION}",
    "docker_cli": "${DOCKER_CLI_VERSION}",
    "linux_virt": "${LINUX_VIRT_VERSION}",
    "mkinitfs": "${MKINITFS_VERSION}",
    "python3": "${PYTHON3_VERSION}",
    "e2fsprogs": "${E2FSPROGS_VERSION}",
    "iproute2": "${IPROUTE2_VERSION}",
    "util_linux": "${UTIL_LINUX_VERSION}",
    "bubblewrap": "${BUBBLEWRAP_VERSION}",
    "nodejs": "${NODEJS_VERSION}",
    "npm": "${NPM_VERSION}",
    "mise": "${MISE_VERSION}",
    "mise_target": "${MISE_TARGET}"
  }
}
EOF
}

main() {
  require_root
  require_commands
  require_version_pins
  clean_dirs
  bootstrap_rootfs
  write_apk_repositories
  install_guest_assets
  configure_rootfs
  install_mise_binary
  copy_kernel_artifacts
  pack_rootfs
  write_manifest
  printf 'Built appliance artifacts in %s\n' "${OUT_DIR}"
}

main "$@"
