#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "${SCRIPT_DIR}/../.." && pwd)

fail() {
  echo "test_guest_init.sh: $*" >&2
  exit 1
}

assert_eq() {
  expected="$1"
  actual="$2"
  name="$3"
  [ "${expected}" = "${actual}" ] || fail "${name}: expected '${expected}', got '${actual}'"
}

AGENTVM_GUEST_INIT_SOURCE_ONLY=1 . "${ROOT}/docker/guest-init.sh"

cat > /tmp/agentvm-launch.json <<'EOF'
{
  "schema_version": 1,
  "project_path": "/tmp/project with spaces",
  "network": {
    "guest_ip": "10.0.2.15",
    "gateway_ip": "10.0.2.2",
    "prefix_len": 24,
    "dns_ip": "10.0.2.3",
    "guest_mac": "02:fc:12:34:56:78"
  },
  "http_smoke_url": "http://198.51.100.10/"
}
EOF
AGENTVM_LAUNCH_CONFIG=/tmp/agentvm-launch.json load_launch_config
assert_eq "/tmp/project with spaces" "${PROJECT_PATH}" "launch config project path"
assert_eq 10.0.2.15 "${GUEST_IP}" "launch config guest IP"
assert_eq 10.0.2.2 "${GATEWAY_IP}" "launch config gateway IP"
assert_eq 24 "${PREFIX_LEN}" "launch config prefix length"
assert_eq 10.0.2.3 "${DNS_IP}" "launch config DNS IP"
assert_eq 02:fc:12:34:56:78 "${GUEST_MAC}" "launch config guest MAC"
assert_eq http://198.51.100.10/ "${HTTP_SMOKE_URL}" "launch config HTTP smoke URL"

cat > /tmp/agentvm-invalid-launch.json <<'EOF'
{"schema_version": 1, "project_path": "/workspace", "network": {}}
EOF
if AGENTVM_LAUNCH_CONFIG=/tmp/agentvm-invalid-launch.json load_launch_config >/tmp/agentvm-invalid-launch-log 2>&1; then
  fail "invalid launch config unexpectedly succeeded"
fi
if ! grep -q 'failed to parse launch config' /tmp/agentvm-invalid-launch-log; then
  fail "invalid launch config failure was not logged"
fi

AGENTVM_ROOT_OVERLAY_READY=1
mount() { fail "setup_root_overlay should not mount when overlay is already ready"; }
setup_root_overlay
unset AGENTVM_ROOT_OVERLAY_READY

# Mock mutating commands for bind_composed_entry. The function should reject
# invalid absolute-path contracts before trying to create or mount anything.
mkdir() { fail "mkdir should not be called for invalid bind"; }
mount() { fail "mount should not be called for invalid bind"; }

if bind_composed_entry dir relative /target true true >/dev/null; then
  fail "required invalid source unexpectedly succeeded"
fi
bind_composed_entry dir relative /target false true >/dev/null

calls=/tmp/agentvm-bind-calls
: >"${calls}"
mkdir() { printf 'mkdir %s\n' "$*" >>"${calls}"; }
mount() { printf 'mount %s\n' "$*" >>"${calls}"; }
bind_composed_entry dir /source/dir /target/dir true true >/dev/null
if ! grep -q 'mkdir -p /target' "${calls}"; then
  fail "bind_composed_entry did not create parent for dir target"
fi
if ! grep -q 'mkdir -p /target/dir' "${calls}"; then
  fail "bind_composed_entry did not create dir target"
fi
if ! grep -q 'mount --bind /source/dir /target/dir' "${calls}"; then
  fail "bind_composed_entry did not mount valid dir bind"
fi

kill_count=/tmp/agentvm-kill-count
: >"${kill_count}"
DOCKERD_PID=101
BRIDGE_PID=102
PAYLOAD_SERVER_PID=103
kill() {
  if [ "$1" = "-0" ]; then
    printf '%s\n' "$2" >>"${kill_count}"
    [ "$2" != "102" ]
    return
  fi
  command kill "$@"
}
sleep() { :; }
wait_for_critical_exit
assert_eq "101
102" "$(cat "${kill_count}")" "critical service wait order"

assert_eq "/usr/local/libexec/agentvm-guest-service" "$(payload_server_command)" "payload command"
assert_eq "vfs" "${DOCKER_STORAGE_DRIVER}" "default Docker storage driver"
if ! grep -F -- '--storage-driver="${DOCKER_STORAGE_DRIVER}"' "${ROOT}/docker/guest-init.sh" >/dev/null; then
  fail "dockerd command does not pin the configured storage driver"
fi

ping_count=/tmp/agentvm-docker-ping-count
: >"${ping_count}"
docker_socket_exists() { [ "$1" = /var/run/docker.sock ]; }
docker_socket_ping() {
  count=$(cat "${ping_count}")
  count=$((count + 1))
  printf '%s\n' "${count}" >"${ping_count}"
  [ "${count}" -ge 3 ]
}
wait_for_docker_ready /var/run/docker.sock 101 5 0 >/tmp/agentvm-docker-ready-log
assert_eq 3 "$(cat "${ping_count}")" "docker readiness retries until ping succeeds"
if ! grep -q 'Docker socket is ready' /tmp/agentvm-docker-ready-log; then
  fail "docker readiness success was not logged"
fi

: >"${ping_count}"
docker_socket_ping() { return 1; }
if wait_for_docker_ready /var/run/docker.sock 102 5 0 >/tmp/agentvm-docker-exit-log; then
  fail "docker readiness unexpectedly succeeded after dockerd exit"
fi
if ! grep -q 'dockerd exited before Docker socket became ready' /tmp/agentvm-docker-exit-log; then
  fail "dockerd exit readiness failure was not logged"
fi

rm -f /tmp/agentvm-launch.json /tmp/agentvm-invalid-launch.json /tmp/agentvm-invalid-launch-log "${calls}" "${kill_count}" "${ping_count}" /tmp/agentvm-docker-ready-log /tmp/agentvm-docker-exit-log
