#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

usage() {
  cat <<'EOF'
usage: vm-frontend/validate.sh <tier>

tiers:
  fast        Run normal offline Rust tests for composed-fs and vm-frontend.
  stress      Run opt-in ignored stress/property tests.
  all-local   Run fast and stress tiers.
  live        Run the KVM/QEMU self-test. Requires /dev/kvm and built appliance artifacts.

live environment overrides:
  QEMU=/usr/bin/qemu-system-x86_64
  IMAGE=alpine:3.22
  PUBLISH_PAYLOAD_PORT=12079
EOF
}

fast() {
  cargo test --manifest-path composed-fs/Cargo.toml --offline
  cargo test --manifest-path vm-frontend/Cargo.toml --offline
}

stress() {
  cargo test --manifest-path composed-fs/Cargo.toml --offline \
    proptest_flat_file_operation_sequences -- --ignored --nocapture
  cargo test --manifest-path composed-fs/Cargo.toml --offline \
    stress_seeded_flat_file_operation_sequences -- --ignored --nocapture
  cargo test --manifest-path vm-frontend/Cargo.toml --offline \
    dns_proxy_stress -- --ignored --nocapture
}

live() {
  if [ ! -e /dev/kvm ]; then
    echo "error: /dev/kvm is not available; live validation must run on the host" >&2
    exit 1
  fi
  local qemu="${QEMU:-/usr/bin/qemu-system-x86_64}"
  local image="${IMAGE:-alpine:3.22}"
  local publish_port="${PUBLISH_PAYLOAD_PORT:-12079}"
  cargo run --manifest-path vm-frontend/Cargo.toml --offline -- \
    self-test \
    --project "${ROOT}" \
    --run-dir "${ROOT}/.sandbox/docker-vm/self-test" \
    --artifact-manifest "${ROOT}/docker/out/artifact-manifest.json" \
    --qemu "${qemu}" \
    --image "${image}" \
    --publish-payload-port "${publish_port}"
}

case "${1:-}" in
  fast) fast ;;
  stress) stress ;;
  all-local)
    fast
    stress
    ;;
  live) live ;;
  -h|--help|"")
    usage
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
