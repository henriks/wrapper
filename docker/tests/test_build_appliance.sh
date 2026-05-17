#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "${ROOT}"

fail() {
  echo "test_build_appliance: $*" >&2
  exit 1
}

tmpdir=$(mktemp -d)
repo_optional="guest-service/target/test-agentvm-guest-service"
trap 'rm -rf "${tmpdir}"; rm -f "${repo_optional}"' EXIT

mkdir -p "$(dirname "${repo_optional}")"
printf '#!/bin/sh\necho guest-service\n' >"${repo_optional}"
chmod 0755 "${repo_optional}"

json=$(
  AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 \
  AGENTVM_GUEST_SERVICE_BIN="${repo_optional}" \
  bash -c 'source docker/build-appliance.sh; source_inputs_json'
)
echo "${json}" | grep -F '"path": "guest-service/target/test-agentvm-guest-service"' >/dev/null || \
  fail "optional guest-service binary was not recorded in source_inputs"
expected_hash=$(sha256sum "${repo_optional}" | awk '{print $1}')
echo "${json}" | grep -F '"sha256": "'"${expected_hash}"'"' >/dev/null || \
  fail "optional guest-service binary hash was not recorded in source_inputs"

if AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 \
   AGENTVM_GUEST_SERVICE_BIN="${tmpdir}/missing" \
   bash -c 'source docker/build-appliance.sh; require_optional_guest_service_binary' 2>/dev/null; then
  fail "missing optional guest-service binary was accepted"
fi

if AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 \
   AGENTVM_PAYLOAD_SERVICE=rust \
   bash -c 'source docker/build-appliance.sh; require_optional_guest_service_binary' 2>/dev/null; then
  fail "rust payload service without guest-service binary was accepted"
fi

cmdline=$(
  AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 \
  AGENTVM_PAYLOAD_SERVICE=rust \
  AGENTVM_GUEST_SERVICE_BIN="${repo_optional}" \
  bash -c 'source docker/build-appliance.sh; kernel_cmdline'
)
echo "${cmdline}" | grep -F 'agentvm_payload_service=rust' >/dev/null || \
  fail "rust payload service was not added to kernel cmdline"

cmdline=$(
  AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 \
  AGENTVM_GUEST_SERVICE_BIN="${repo_optional}" \
  bash -c 'source docker/build-appliance.sh; kernel_cmdline'
)
if echo "${cmdline}" | grep -F 'agentvm_payload_service=' >/dev/null; then
  fail "python default unexpectedly added payload service kernel arg"
fi

echo "test_build_appliance: ok"
