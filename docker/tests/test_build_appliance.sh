#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "${ROOT}"

fail() {
  echo "test_build_appliance: $*" >&2
  exit 1
}

tmpdir=$(mktemp -d)
repo_guest_service="guest-service/target/test-agentvm-guest-service"
guest_service_main="guest-service/src/main.rs"
guest_service_main_timestamp="${tmpdir}/guest-service-main.timestamp"
touch -r "${guest_service_main}" "${guest_service_main_timestamp}"
cleanup() {
  touch -r "${guest_service_main_timestamp}" "${guest_service_main}" 2>/dev/null || true
  rm -rf "${tmpdir}"
  rm -f "${repo_guest_service}"
}
trap cleanup EXIT

mkdir -p "$(dirname "${repo_guest_service}")"
printf 'fake musl elf\n' >"${repo_guest_service}"
chmod 0755 "${repo_guest_service}"

fake_readelf_dir="${tmpdir}/bin"
mkdir -p "${fake_readelf_dir}"
cat >"${fake_readelf_dir}/readelf" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
mode=$1
binary=$2
case "${mode}" in
  -h)
    case "${binary}" in
      *musl-guest-service|*glibc-guest-service|*test-agentvm-guest-service) echo 'ELF Header:' ;;
      *) exit 1 ;;
    esac
    ;;
  -l)
    case "${binary}" in
      *musl-guest-service|*test-agentvm-guest-service) echo '      [Requesting program interpreter: /lib/ld-musl-x86_64.so.1]' ;;
      *glibc-guest-service) echo '      [Requesting program interpreter: /lib64/ld-linux-x86-64.so.2]' ;;
      *) exit 1 ;;
    esac
    ;;
  *) exit 1 ;;
esac
EOF
chmod 0755 "${fake_readelf_dir}/readelf"
musl_guest_service="${tmpdir}/musl-guest-service"
glibc_guest_service="${tmpdir}/glibc-guest-service"
non_elf_guest_service="${tmpdir}/non-elf-guest-service"
printf 'fake musl elf\n' >"${musl_guest_service}"
printf 'fake glibc elf\n' >"${glibc_guest_service}"
printf '#!/bin/sh\necho not-elf\n' >"${non_elf_guest_service}"
chmod 0755 "${musl_guest_service}" "${glibc_guest_service}" "${non_elf_guest_service}"

json=$(
  PATH="${fake_readelf_dir}:${PATH}" \
  AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 \
  AGENTVM_GUEST_SERVICE_BIN="${repo_guest_service}" \
  bash -c 'source docker/build-appliance.sh; require_guest_service_binary; source_inputs_json'
)
echo "${json}" | grep -F '"path": "guest-service/target/test-agentvm-guest-service"' >/dev/null || \
  fail "guest-service binary was not recorded in source_inputs"
echo "${json}" | grep -F '"path": "guest-service/src/main.rs"' >/dev/null || \
  fail "guest-service source was not recorded in source_inputs"
echo "${json}" | grep -F '"path": "payload-protocol/src/lib.rs"' >/dev/null || \
  fail "payload-protocol source was not recorded in source_inputs"
expected_hash=$(sha256sum "${repo_guest_service}" | awk '{print $1}')
echo "${json}" | grep -F '"sha256": "'"${expected_hash}"'"' >/dev/null || \
  fail "guest-service binary hash was not recorded in source_inputs"

touch -d 'next hour' "${guest_service_main}"
if PATH="${fake_readelf_dir}:${PATH}" \
   AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 \
   AGENTVM_GUEST_SERVICE_BIN="${repo_guest_service}" \
   bash -c 'source docker/build-appliance.sh; require_guest_service_binary' 2>/dev/null; then
  fail "stale guest-service binary was accepted"
fi
touch -r "${guest_service_main_timestamp}" "${guest_service_main}"

if AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 \
   AGENTVM_GUEST_SERVICE_BIN="${tmpdir}/missing" \
   bash -c 'source docker/build-appliance.sh; require_guest_service_binary' 2>/dev/null; then
  fail "missing guest-service binary was accepted"
fi

if AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 \
   bash -c 'source docker/build-appliance.sh; require_guest_service_binary' 2>/dev/null; then
  fail "unset guest-service binary was accepted"
fi

if AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 \
   AGENTVM_GUEST_SERVICE_BIN="${non_elf_guest_service}" \
   bash -c 'source docker/build-appliance.sh; require_guest_service_binary' 2>/dev/null; then
  fail "non-ELF guest-service binary was accepted"
fi

if PATH="${fake_readelf_dir}:${PATH}" \
   AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 \
   AGENTVM_GUEST_SERVICE_BIN="${glibc_guest_service}" \
   bash -c 'source docker/build-appliance.sh; require_guest_service_binary' 2>/dev/null; then
  fail "glibc-linked guest-service binary was accepted"
fi

if ! PATH="${fake_readelf_dir}:${PATH}" \
   AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 \
   AGENTVM_GUEST_SERVICE_BIN="${musl_guest_service}" \
   bash -c 'source docker/build-appliance.sh; require_guest_service_binary' 2>/dev/null; then
  fail "musl-compatible guest-service binary was rejected"
fi

env_text=$(
  PATH="${fake_readelf_dir}:${PATH}" \
  AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 \
  AGENTVM_GUEST_SERVICE_BIN="${musl_guest_service}" \
  bash -c 'source docker/build-appliance.sh; require_guest_service_binary; agentvm_env_contents'
)
if echo "${env_text}" | grep -F 'PAYLOAD_SERVICE=' >/dev/null; then
  fail "payload service selector was written to agentvm env"
fi
if ! echo "${env_text}" | grep -Fx 'DOCKER_STORAGE_DRIVER=vfs' >/dev/null; then
  fail "Docker storage driver was not written to agentvm env"
fi

cmdline=$(
  PATH="${fake_readelf_dir}:${PATH}" \
  AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 \
  AGENTVM_GUEST_SERVICE_BIN="${musl_guest_service}" \
  bash -c 'source docker/build-appliance.sh; require_guest_service_binary; kernel_cmdline'
)
if echo "${cmdline}" | grep -F 'agentvm_payload_service=' >/dev/null; then
  fail "payload service unexpectedly added kernel arg"
fi

if grep -E 'BUBBLEWRAP_VERSION|bubblewrap=' \
   docker/appliance.env docker/build-appliance.sh docker/refresh-pins.sh docker/README.md >/dev/null; then
  fail "bubblewrap appliance dependency is still pinned, installed, refreshed, or documented"
fi

echo "test_build_appliance: ok"
