#!/bin/sh
set -eu

if [ "${AGENTVM_GUEST_INIT_SOURCE_ONLY:-0}" = "1" ] && [ ! -f /etc/agentvm.env ]; then
  DOCKER_TCP_PORT=1075
  PAYLOAD_TCP_PORT=1076
  VIRTIOFS_TAG=workspace
  CONFIG_VIRTIOFS_TAG=agentvm-config
  ROOT_OVERLAY_LOWER_DEVICE=/dev/vda
  ROOT_OVERLAY_STATE_DEVICE=/dev/vdb
else
  . /etc/agentvm.env
fi

ROOT_OVERLAY_LOWER_DEVICE=${ROOT_OVERLAY_LOWER_DEVICE:-/dev/vda}
ROOT_OVERLAY_STATE_DEVICE=${ROOT_OVERLAY_STATE_DEVICE:-/dev/vdb}

readonly DOCKER_TCP_PORT
readonly PAYLOAD_TCP_PORT
readonly VIRTIOFS_TAG
readonly CONFIG_VIRTIOFS_TAG
readonly ROOT_OVERLAY_LOWER_DEVICE
readonly ROOT_OVERLAY_STATE_DEVICE
readonly GUEST_DOCKERD_LOG=/run/dockerd.log
readonly GUEST_SOCKET_BRIDGE_LOG=/run/socket-bridge.log
readonly GUEST_PAYLOAD_SERVER_LOG=/run/payload-server.log

log() {
  echo "agentvm-init: $*"
}

wait_for_block_device() {
  device="$1"
  attempts=0
  while [ ! -b "${device}" ]; do
    attempts=$((attempts + 1))
    if [ "${attempts}" -ge 50 ]; then
      log "error: block device did not appear: ${device}"
      exit 1
    fi
    sleep 0.1
  done
}

setup_root_overlay() {
  [ "${AGENTVM_ROOT_OVERLAY_READY:-0}" = "1" ] && return 0

  log "setting up persistent root overlay using ${ROOT_OVERLAY_STATE_DEVICE}"
  mount -t devtmpfs devtmpfs /dev 2>/dev/null || true
  mount -t tmpfs tmpfs /run 2>/dev/null || true
  wait_for_block_device "${ROOT_OVERLAY_LOWER_DEVICE}"
  wait_for_block_device "${ROOT_OVERLAY_STATE_DEVICE}"
  mkdir -p /run/agentvm-lower /run/agentvm-state /run/agentvm-newroot

  if ! mount -o ro "${ROOT_OVERLAY_LOWER_DEVICE}" /run/agentvm-lower; then
    log "warning: failed to mount immutable root ${ROOT_OVERLAY_LOWER_DEVICE}; using current root as overlay lower"
    mount --bind / /run/agentvm-lower || {
      log "error: failed to bind current root as overlay lower"
      exit 1
    }
  fi
  mount "${ROOT_OVERLAY_STATE_DEVICE}" /run/agentvm-state || {
    log "error: failed to mount root overlay state ${ROOT_OVERLAY_STATE_DEVICE}"
    exit 1
  }
  mkdir -p /run/agentvm-state/root/upper /run/agentvm-state/root/work
  modprobe overlay 2>/dev/null || true
  mount -t overlay overlay \
    -o lowerdir=/run/agentvm-lower,upperdir=/run/agentvm-state/root/upper,workdir=/run/agentvm-state/root/work \
    /run/agentvm-newroot || {
    log "error: failed to mount persistent root overlay"
    exit 1
  }
  mkdir -p /run/agentvm-newroot/proc
  mount -t proc proc /run/agentvm-newroot/proc 2>/dev/null || true

  exec env AGENTVM_ROOT_OVERLAY_READY=1 \
    chroot /run/agentvm-newroot /usr/local/sbin/agentvm-init
}

get_cmdline_value() {
  key="$1"
  for arg in $(cat /proc/cmdline); do
    case "${arg}" in
      "${key}"=*)
        printf '%s\n' "${arg#${key}=}"
        return 0
        ;;
    esac
  done
  return 1
}

find_iface_by_mac() {
  mac="$1"
  for path in /sys/class/net/*/address; do
    [ -f "${path}" ] || continue
    if [ "$(cat "${path}")" = "${mac}" ]; then
      basename "$(dirname "${path}")"
      return 0
    fi
  done
  return 1
}

load_kernel_module() {
  module="$1"
  if modprobe "$module" 2>/dev/null; then
    log "loaded kernel module ${module}"
  else
    log "warning: failed to load kernel module ${module}"
  fi
}

install_mitm_ca() {
  ca_path=/run/agentvm-config/mitm-ca.crt
  [ -f "${ca_path}" ] || return 0

  bundle=/run/agentvm-ca-bundle.pem
  : >"${bundle}"
  if [ -f /etc/ssl/cert.pem ]; then
    cat /etc/ssl/cert.pem >>"${bundle}"
  elif [ -f /etc/ssl/certs/ca-certificates.crt ]; then
    cat /etc/ssl/certs/ca-certificates.crt >>"${bundle}"
  fi
  printf '\n' >>"${bundle}"
  cat "${ca_path}" >>"${bundle}"

  export SSL_CERT_FILE="${bundle}"
  export REQUESTS_CA_BUNDLE="${bundle}"
  log "installed MITM CA bundle at ${bundle}"
}

bind_composed_entry() {
  kind="$1"
  source="$2"
  target="$3"
  required="$4"
  create_parent="$5"

  case "${source}:${target}" in
    /*:/*) ;;
    *)
      log "warning: ignoring invalid composed bind ${source} -> ${target}"
      [ "${required}" = "true" ] && return 1
      return 0
      ;;
  esac

  if [ "${create_parent}" = "true" ]; then
    mkdir -p "$(dirname "${target}")"
  fi

  if [ "${kind}" = "dir" ]; then
    mkdir -p "${target}"
  elif [ "${kind}" = "file" ]; then
    if [ -d "${target}" ]; then
      log "warning: composed file bind target is a directory: ${target}"
      [ "${required}" = "true" ] && return 1
      return 0
    fi
    [ -e "${target}" ] || : > "${target}"
  else
    log "warning: unknown composed bind kind ${kind} for ${target}"
    [ "${required}" = "true" ] && return 1
    return 0
  fi

  if mount --bind "${source}" "${target}"; then
    return 0
  fi

  log "warning: failed composed bind ${source} -> ${target}"
  [ "${required}" = "true" ] && return 1
  return 0
}

mount_composed_export() {
  manifest="/run/agentvm-config/composed-binds.json"
  mountpoint=$(python3 - "${manifest}" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    manifest = json.load(f)
print(manifest.get("composed_mountpoint", "/run/agentvm-host"))
PY
)
  mkdir -p "${mountpoint}"
  mount -t virtiofs "${VIRTIOFS_TAG}" "${mountpoint}"

  python3 - "${manifest}" <<'PY' | while IFS="$(printf '\t')" read -r kind source target required create_parent; do
import json
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    manifest = json.load(f)

for entry in manifest.get("entries", []):
    print(
        "\t".join([
            entry.get("kind", ""),
            entry.get("source", ""),
            entry.get("target", ""),
            "true" if entry.get("required", False) else "false",
            "true" if entry.get("create_parent", False) else "false",
        ])
    )
PY
    [ -n "${source}" ] || continue
    bind_composed_entry "${kind}" "${source}" "${target}" \
      "${required}" "${create_parent}"
  done
}

mirror_log_to_workspace() {
  src="$1"
  dst="$2"
  (
    while [ ! -f "${src}" ]; do
      sleep 1
    done
    tail -n +1 -f "${src}" >>"${dst}" 2>/dev/null
  ) &
}

dump_log_if_present() {
  path="$1"
  name="$2"
  if [ -f "${path}" ]; then
    echo "agentvm-init: begin ${name}"
    sed 's/^/'"${name}"': /' "${path}" || true
    echo "agentvm-init: end ${name}"
  fi
}

wait_for_critical_exit() {
  while :; do
    for pid in "${DOCKERD_PID}" "${BRIDGE_PID}" "${PAYLOAD_SERVER_PID}"; do
      if ! kill -0 "${pid}" 2>/dev/null; then
        return 0
      fi
    done
    sleep 1
  done
}

teardown() {
  if [ -n "${PAYLOAD_SERVER_PID:-}" ]; then
    kill "${PAYLOAD_SERVER_PID}" 2>/dev/null || true
  fi
  if [ -n "${BRIDGE_PID:-}" ]; then
    kill "${BRIDGE_PID}" 2>/dev/null || true
  fi
  if [ -n "${DOCKERD_PID:-}" ]; then
    kill "${DOCKERD_PID}" 2>/dev/null || true
  fi
  wait || true
  poweroff -f || reboot -f || true
}

main() {
trap teardown INT TERM HUP
setup_root_overlay

PROJECT_PATH=$(get_cmdline_value agentvm_project || true)
GUEST_IP=$(get_cmdline_value agentvm_guest_ip || true)
GATEWAY_IP=$(get_cmdline_value agentvm_gateway_ip || true)
PREFIX_LEN=$(get_cmdline_value agentvm_prefix_len || true)
DNS_IP=$(get_cmdline_value agentvm_dns || true)
GUEST_MAC=$(get_cmdline_value agentvm_guest_mac || true)
HTTP_SMOKE_URL=$(get_cmdline_value agentvm_http_smoke_url || true)
if [ -z "${PROJECT_PATH}" ]; then
  PROJECT_PATH=/workspace
fi
case "${PROJECT_PATH}" in
  /*) ;;
  *)
    log "warning: invalid project path '${PROJECT_PATH}', falling back to /workspace"
    PROJECT_PATH=/workspace
    ;;
esac

mkdir -p /proc /sys /dev /dev/pts /run /tmp /home /workspace /var/lib/docker /var/log /sys/fs/cgroup
mount -t proc proc /proc || true
mount -t sysfs sysfs /sys || true
mount -t devtmpfs devtmpfs /dev || true
mount -t devpts devpts /dev/pts || true
ln -sf /dev/pts/ptmx /dev/ptmx
mount -t cgroup2 none /sys/fs/cgroup || true
mount -t tmpfs tmpfs /run
mount -t tmpfs tmpfs /tmp
mkdir -p /run/agentvm-config /run/agentvm-share-mnts
mount -t virtiofs "${CONFIG_VIRTIOFS_TAG}" /run/agentvm-config

case "${PROJECT_PATH}" in
  /home/*|/tmp/*|/workspace)
    ;;
  *)
    log "warning: unsupported guest project path '${PROJECT_PATH}', falling back to /workspace"
    PROJECT_PATH=/workspace
    ;;
esac

HOST_RUN_DIR=${PROJECT_PATH}/.sandbox/docker-vm/run
HOST_DOCKERD_LOG=${HOST_RUN_DIR}/guest-dockerd.log
HOST_SOCKET_BRIDGE_LOG=${HOST_RUN_DIR}/guest-socket-bridge.log
HOST_PAYLOAD_SERVER_LOG=${HOST_RUN_DIR}/guest-payload-server.log
readonly \
  PROJECT_PATH \
  HOST_RUN_DIR \
  HOST_DOCKERD_LOG \
  HOST_SOCKET_BRIDGE_LOG \
  HOST_PAYLOAD_SERVER_LOG

[ -f /run/agentvm-config/composed-binds.json ] || {
  log "error: missing composed bind manifest"
  exit 1
}
mount_composed_export
install_mitm_ca
if [ "${PROJECT_PATH}" != "/workspace" ]; then
  mount --bind "${PROJECT_PATH}" /workspace
fi
mkdir -p "${HOST_RUN_DIR}"
touch "${HOST_DOCKERD_LOG}" "${HOST_SOCKET_BRIDGE_LOG}" "${HOST_PAYLOAD_SERVER_LOG}"
mirror_log_to_workspace "${GUEST_DOCKERD_LOG}" "${HOST_DOCKERD_LOG}"
mirror_log_to_workspace "${GUEST_SOCKET_BRIDGE_LOG}" "${HOST_SOCKET_BRIDGE_LOG}"
mirror_log_to_workspace "${GUEST_PAYLOAD_SERVER_LOG}" "${HOST_PAYLOAD_SERVER_LOG}"

modprobe overlay || true
load_kernel_module virtio_net
ip link set lo up || true

if [ -n "${GUEST_IP}" ] && [ -n "${GATEWAY_IP}" ] && [ -n "${PREFIX_LEN}" ] && [ -n "${GUEST_MAC}" ]; then
  IFACE=$(find_iface_by_mac "${GUEST_MAC}" || true)
  if [ -n "${IFACE}" ]; then
    ip link set "${IFACE}" up
    ip addr add "${GUEST_IP}/${PREFIX_LEN}" dev "${IFACE}" || true
    ip route replace default via "${GATEWAY_IP}" dev "${IFACE}" || true
    if [ -n "${DNS_IP}" ]; then
      printf 'nameserver %s\n' "${DNS_IP}" >/run/resolv.conf
    fi
    log "configured network on ${IFACE} (${GUEST_IP}/${PREFIX_LEN} via ${GATEWAY_IP})"
  else
    log "warning: failed to locate guest network interface for ${GUEST_MAC}"
  fi
fi

if [ -n "${HTTP_SMOKE_URL}" ]; then
  log "running HTTP smoke request to ${HTTP_SMOKE_URL}"
  if wget -S -O /run/agentvm-http-smoke.out "${HTTP_SMOKE_URL}" \
      >"${HOST_RUN_DIR}/guest-http-smoke.log" 2>&1; then
    log "HTTP smoke request completed"
  else
    log "warning: HTTP smoke request failed"
  fi
fi

log "starting dockerd"
dockerd \
  --host=unix:///var/run/docker.sock \
  --data-root=/var/lib/docker \
  --exec-root=/run/docker \
  >"${GUEST_DOCKERD_LOG}" 2>&1 &
DOCKERD_PID=$!

log "starting socket bridge"
python3 -u /usr/local/libexec/agentvm-socket-bridge \
  --tcp-host 0.0.0.0 \
  --tcp-port "${DOCKER_TCP_PORT}" \
  --docker-sock /var/run/docker.sock \
  >"${GUEST_SOCKET_BRIDGE_LOG}" 2>&1 &
BRIDGE_PID=$!

log "starting payload server"
python3 -u /usr/local/libexec/agentvm-payload-server \
  --tcp-host 0.0.0.0 \
  --tcp-port "${PAYLOAD_TCP_PORT}" \
  >"${GUEST_PAYLOAD_SERVER_LOG}" 2>&1 &
PAYLOAD_SERVER_PID=$!

wait_for_critical_exit
log "critical service exited"
dump_log_if_present "${GUEST_DOCKERD_LOG}" dockerd.log
dump_log_if_present "${GUEST_SOCKET_BRIDGE_LOG}" socket-bridge.log
dump_log_if_present "${GUEST_PAYLOAD_SERVER_LOG}" payload-server.log
teardown

}

if [ "${AGENTVM_GUEST_INIT_SOURCE_ONLY:-0}" = "1" ]; then
  return 0 2>/dev/null || exit 0
fi

main "$@"
