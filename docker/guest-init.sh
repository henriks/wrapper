#!/bin/sh
set -eu

. /etc/agentvm.env

readonly VSOCK_PORT
readonly VIRTIOFS_TAG
readonly GUEST_DOCKERD_LOG=/run/dockerd.log
readonly GUEST_VSOCK_BRIDGE_LOG=/run/vsock-bridge.log

log() {
  echo "agentvm-init: $*"
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

load_kernel_module() {
  module="$1"
  if modprobe "$module" 2>/dev/null; then
    log "loaded kernel module ${module}"
  else
    log "warning: failed to load kernel module ${module}"
  fi
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
    for pid in "${DOCKERD_PID}" "${BRIDGE_PID}"; do
      if ! kill -0 "${pid}" 2>/dev/null; then
        return 0
      fi
    done
    sleep 1
  done
}

teardown() {
  if [ -n "${BRIDGE_PID:-}" ]; then
    kill "${BRIDGE_PID}" 2>/dev/null || true
  fi
  if [ -n "${DOCKERD_PID:-}" ]; then
    kill "${DOCKERD_PID}" 2>/dev/null || true
  fi
  wait || true
  poweroff -f || reboot -f || true
}

trap teardown INT TERM HUP

PROJECT_PATH=$(get_cmdline_value agentvm_project || true)
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

mkdir -p /proc /sys /dev /run /tmp /workspace /var/lib/docker /var/log /sys/fs/cgroup
mount -t proc proc /proc || true
mount -t sysfs sysfs /sys || true
mount -t devtmpfs devtmpfs /dev || true
mount -t cgroup2 none /sys/fs/cgroup || true
mount -t tmpfs tmpfs /run
mount -t tmpfs tmpfs /tmp
mount /dev/vdb /var/lib/docker

case "${PROJECT_PATH}" in
  /home/*)
    mount -t tmpfs tmpfs /home || true
    ;;
  /tmp/*|/workspace)
    ;;
  *)
    log "warning: unsupported guest project path '${PROJECT_PATH}', falling back to /workspace"
    PROJECT_PATH=/workspace
    ;;
esac

HOST_RUN_DIR=${PROJECT_PATH}/.sandbox/docker-vm/run
HOST_DOCKERD_LOG=${HOST_RUN_DIR}/guest-dockerd.log
HOST_VSOCK_BRIDGE_LOG=${HOST_RUN_DIR}/guest-vsock-bridge.log
readonly PROJECT_PATH HOST_RUN_DIR HOST_DOCKERD_LOG HOST_VSOCK_BRIDGE_LOG

mkdir -p "${PROJECT_PATH}"
mount -t virtiofs "${VIRTIOFS_TAG}" "${PROJECT_PATH}"
if [ "${PROJECT_PATH}" != "/workspace" ]; then
  mount --bind "${PROJECT_PATH}" /workspace
fi
mkdir -p "${HOST_RUN_DIR}"
touch "${HOST_DOCKERD_LOG}" "${HOST_VSOCK_BRIDGE_LOG}"
mirror_log_to_workspace "${GUEST_DOCKERD_LOG}" "${HOST_DOCKERD_LOG}"
mirror_log_to_workspace "${GUEST_VSOCK_BRIDGE_LOG}" "${HOST_VSOCK_BRIDGE_LOG}"

modprobe overlay || true
load_kernel_module vsock
load_kernel_module vmw_vsock_virtio_transport_common
load_kernel_module vmw_vsock_virtio_transport

log "starting dockerd"
dockerd \
  --host=unix:///var/run/docker.sock \
  --data-root=/var/lib/docker \
  --exec-root=/run/docker \
  >"${GUEST_DOCKERD_LOG}" 2>&1 &
DOCKERD_PID=$!

log "starting vsock bridge"
python3 -u /usr/local/libexec/agentvm-vsock-bridge \
  --vsock-port "${VSOCK_PORT}" \
  --docker-sock /var/run/docker.sock \
  >"${GUEST_VSOCK_BRIDGE_LOG}" 2>&1 &
BRIDGE_PID=$!

wait_for_critical_exit
log "critical service exited"
dump_log_if_present "${GUEST_DOCKERD_LOG}" dockerd.log
dump_log_if_present "${GUEST_VSOCK_BRIDGE_LOG}" vsock-bridge.log
teardown
