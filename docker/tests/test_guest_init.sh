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

cat > /tmp/agentvm-test-cmdline <<'EOF'
console=hvc0 agentvm_project=/workspace agentvm_guest_ip=10.0.2.15 agentvm_dns=10.0.2.3
EOF
cat() {
  if [ "$1" = /proc/cmdline ]; then
    command cat /tmp/agentvm-test-cmdline
  else
    command cat "$@"
  fi
}

assert_eq /workspace "$(get_cmdline_value agentvm_project)" "project cmdline value"
assert_eq 10.0.2.3 "$(get_cmdline_value agentvm_dns)" "dns cmdline value"
if get_cmdline_value missing_key >/tmp/agentvm-missing-value; then
  fail "missing cmdline key unexpectedly succeeded"
fi

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

rm -f /tmp/agentvm-test-cmdline /tmp/agentvm-missing-value "${calls}" "${kill_count}"
