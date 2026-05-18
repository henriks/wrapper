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
readonly GUEST_SERVICE_PATH="${ROOTFS_DIR}/usr/local/libexec/agentvm-guest-service"
readonly MINIROOTFS_TARBALL="${BUILD_DIR}/alpine-minirootfs.tar.gz"
readonly GUEST_SERVICE_BIN="${AGENTVM_GUEST_SERVICE_BIN:-}"
readonly REPO_ROOT="${SCRIPT_DIR}/.."

GUEST_SERVICE_SOURCE_FILES=(
  Cargo.toml
  Cargo.lock
  guest-service/Cargo.toml
  guest-service/src/lib.rs
  guest-service/src/main.rs
  payload-protocol/Cargo.toml
  payload-protocol/src/lib.rs
)

require_root() {
  if [[ ${EUID} -ne 0 ]]; then
    echo "error: docker/build-appliance.sh must run as root" >&2
    exit 1
  fi
}

require_commands() {
  local missing=()
  local cmd
  for cmd in chroot curl tar truncate mkfs.ext4 sha256sum realpath; do
    if ! command -v "${cmd}" >/dev/null 2>&1; then
      missing+=("${cmd}")
    fi
  done
  if ((${#missing[@]} > 0)); then
    printf 'error: missing required commands: %s\n' "${missing[*]}" >&2
    exit 1
  fi
}

require_guest_service_binary() {
  if [[ -z "${GUEST_SERVICE_BIN}" ]]; then
    echo "error: AGENTVM_GUEST_SERVICE_BIN is required" >&2
    echo "hint: build with: cargo build --target x86_64-unknown-linux-musl --bin agentvm-guest-service" >&2
    return 1
  fi
  if [[ ! -f "${GUEST_SERVICE_BIN}" ]]; then
    echo "error: AGENTVM_GUEST_SERVICE_BIN does not name a file: ${GUEST_SERVICE_BIN}" >&2
    return 1
  fi
  if [[ ! -x "${GUEST_SERVICE_BIN}" ]]; then
    echo "error: AGENTVM_GUEST_SERVICE_BIN must be executable: ${GUEST_SERVICE_BIN}" >&2
    return 1
  fi
  require_guest_compatible_rust_service_binary "${GUEST_SERVICE_BIN}"
  require_guest_service_binary_fresh "${GUEST_SERVICE_BIN}"
}

repo_relative_path() {
  local path=$1
  local relative
  relative=$(realpath --relative-to="${REPO_ROOT}" "${path}")
  case "${relative}" in
    ../*|/*) return 1 ;;
    *) printf '%s\n' "${relative}" ;;
  esac
}

require_guest_service_binary_fresh() {
  local binary=$1
  local relative
  if ! relative=$(repo_relative_path "${binary}"); then
    return 0
  fi

  local source
  local source_path
  for source in "${GUEST_SERVICE_SOURCE_FILES[@]}"; do
    source_path="${REPO_ROOT}/${source}"
    if [[ ! -f "${source_path}" ]]; then
      echo "error: missing guest-service source input: ${source}" >&2
      return 1
    fi
    if [[ "${source_path}" -nt "${binary}" ]]; then
      echo "error: AGENTVM_GUEST_SERVICE_BIN is stale relative to ${source}" >&2
      echo "error: ${relative} is older than guest-service source inputs" >&2
      echo "hint: rebuild with: cargo build --target x86_64-unknown-linux-musl --bin agentvm-guest-service" >&2
      return 1
    fi
  done
}

require_guest_compatible_rust_service_binary() {
  local binary=$1
  if ! command -v readelf >/dev/null 2>&1; then
    echo "error: readelf is required to validate AGENTVM_GUEST_SERVICE_BIN compatibility" >&2
    return 1
  fi
  if ! readelf -h "${binary}" >/dev/null 2>&1; then
    echo "error: AGENTVM_GUEST_SERVICE_BIN must be an ELF binary for the Alpine/musl guest: ${binary}" >&2
    echo "hint: build with: cargo build --target x86_64-unknown-linux-musl --bin agentvm-guest-service" >&2
    return 1
  fi
  local interpreter
  interpreter=$(readelf -l "${binary}" 2>/dev/null | sed -n 's/.*Requesting program interpreter: \(.*\)]/\1/p' | head -n1)
  case "${interpreter}" in
    ""|/lib/ld-musl-*.so.*) ;;
    *)
      echo "error: AGENTVM_GUEST_SERVICE_BIN is not compatible with the Alpine/musl guest: ${binary}" >&2
      echo "error: unsupported ELF interpreter: ${interpreter}" >&2
      echo "hint: build with: cargo build --target x86_64-unknown-linux-musl --bin agentvm-guest-service" >&2
      return 1
      ;;
  esac
}

require_version_pins() {
  local required=(
    DOCKER_TCP_PORT
    DOCKER_STORAGE_DRIVER
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
    BASH_VERSION
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

agentvm_env_contents() {
  cat <<EOF
DOCKER_TCP_PORT=${DOCKER_TCP_PORT}
DOCKER_STORAGE_DRIVER=${DOCKER_STORAGE_DRIVER}
PAYLOAD_TCP_PORT=${PAYLOAD_TCP_PORT}
VIRTIOFS_TAG=${VIRTIOFS_TAG}
CONFIG_VIRTIOFS_TAG=${CONFIG_VIRTIOFS_TAG}
EOF
}

install_guest_assets() {
  install -d \
    "${ROOTFS_DIR}/etc" \
    "${ROOTFS_DIR}/etc/mkinitfs" \
    "${ROOTFS_DIR}/usr/local/sbin" \
    "${ROOTFS_DIR}/usr/local/libexec"
  install -m 0755 "${SCRIPT_DIR}/guest-init.sh" "${GUEST_INIT_PATH}"
  install -m 0755 "${GUEST_SERVICE_BIN}" "${GUEST_SERVICE_PATH}"
  agentvm_env_contents > "${ROOTFS_DIR}/etc/agentvm.env"
  cat > "${ROOTFS_DIR}/etc/mkinitfs/mkinitfs.conf" <<'EOF'
features="base virtio ext4"
EOF
  install -d \
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
  run_in_chroot "apk add --no-cache docker-engine=${DOCKER_ENGINE_VERSION} docker-cli=${DOCKER_CLI_VERSION} linux-virt=${LINUX_VIRT_VERSION} mkinitfs=${MKINITFS_VERSION} python3=${PYTHON3_VERSION} e2fsprogs=${E2FSPROGS_VERSION} iproute2=${IPROUTE2_VERSION} util-linux=${UTIL_LINUX_VERSION} bash=${BASH_VERSION} nodejs=${NODEJS_VERSION} npm=${NPM_VERSION} ca-certificates"
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

source_inputs_json() {
  local files=(
    appliance.env
    build-appliance.sh
    guest-init.sh
    refresh-pins.sh
  )
  local first=true
  local file
  local path
  local hash
  for file in "${files[@]}"; do
    path="docker/${file}"
    hash=$(sha256sum "${REPO_ROOT}/${path}" | awk '{print $1}')
    if [[ "${first}" == true ]]; then
      first=false
    else
      printf ',\n'
    fi
    printf '    { "path": "%s", "sha256": "%s" }' "${path}" "${hash}"
  done

  for path in "${GUEST_SERVICE_SOURCE_FILES[@]}"; do
    hash=$(sha256sum "${REPO_ROOT}/${path}" | awk '{print $1}')
    printf ',\n    { "path": "%s", "sha256": "%s" }' "${path}" "${hash}"
  done

  local guest_service_path
  if ! guest_service_path=$(repo_relative_path "${GUEST_SERVICE_BIN}"); then
    echo "error: AGENTVM_GUEST_SERVICE_BIN must be inside the repository: ${GUEST_SERVICE_BIN}" >&2
    exit 1
  fi
  hash=$(sha256sum "${GUEST_SERVICE_BIN}" | awk '{print $1}')
  printf ',\n    { "path": "%s", "sha256": "%s" }' "${guest_service_path}" "${hash}"
  printf '\n'
}

kernel_cmdline() {
  printf '%s\n' "console=hvc0 root=/dev/vda rootfstype=ext4 ro init=/usr/local/sbin/agentvm-init quiet"
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
  "source_inputs": [
$(source_inputs_json)  ],
  "vm": {
    "cpus": 2,
    "memory_bytes": 2147483648,
    "root_disk_device": "/dev/vda",
    "state_disk_device": "/dev/vdb",
    "virtiofs_tag": "${VIRTIOFS_TAG}",
    "kernel_cmdline": "$(kernel_cmdline)"
  },
  "guest": {
    "docker_socket": "/var/run/docker.sock",
    "docker_tcp_port": ${DOCKER_TCP_PORT},
    "docker_storage_driver": "${DOCKER_STORAGE_DRIVER}",
    "payload_tcp_port": ${PAYLOAD_TCP_PORT},
    "project_mount": "configured project_path",
    "state_disk_role": "root-overlay"
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
    "bash": "${BASH_VERSION}",
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
  require_guest_service_binary
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

if [[ "${AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY:-0}" != "1" ]]; then
  main "$@"
fi
