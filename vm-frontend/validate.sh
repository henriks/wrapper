#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

usage() {
  cat <<'EOF'
usage: vm-frontend/validate.sh <tier>

tiers:
  required    Run the required validation gate: docs drift check, formatting,
              offline tests, fuzz target compilation, then host live validation.
  full        Alias for required.
  fast        Run normal offline Rust tests for composed-fs and vm-frontend.
  fmt         Run rustfmt checks for Rust crates and fuzz targets.
  fuzz-check  Compile/check coverage-guided fuzz targets without running libFuzzer.
  live-smoke   Run the quick KVM/QEMU self-test required by the gate.
  guest-services
              Run offline tests for guest-init and guest Python services.
  docs        Check that AGENTS.md and workflow docs name the required gate.
  stress      Run opt-in ignored stress/property tests.
  all-local   Run docs, fmt, guest-services, fuzz-check, fast, and stress tiers; does not boot QEMU.
  live-smoke  Run the quick KVM/QEMU self-test required by the gate.
  live-hostile
              Run a slower hostile/no-net KVM scenario (uses HOSTILE_IMAGE or IMAGE).
  live-payload
              Run a payload protocol stress KVM scenario with large request/output.
  live-dns    Run allowed and denied DNS resolver KVM scenarios.
  live-docker Run Docker bridge container egress and no-net denial KVM scenarios.
  live-fs     Run composed-fs live/adversarial KVM scenarios twice with the same run-dir.
  live-persistence
              Run root overlay persistence KVM scenarios across relaunches.
  live-full   Run all named live scenarios.
  live        Alias for live-smoke. Requires /dev/kvm and built appliance artifacts.
  host-live   Alias for live-smoke, emphasizing that this tier must run on the KVM host.

live environment overrides:
  KVM_DEVICE=/dev/kvm
  QEMU=/usr/bin/qemu-system-x86_64
  IMAGE=alpine:3.22
  PUBLISH_PAYLOAD_PORT=12079
  HOSTILE_IMAGE=alpine:3.22
  PAYLOAD_IMAGE=alpine:3.22
  DOCKER_IMAGE=alpine:3.22
  DOCKER_PUBLISH_HOST_PORT=12080
  DOCKER_PUBLISH_GUEST_PORT=18080
EOF
}

announce() {
  echo "==> $*"
}

fast() {
  announce "offline tests: composed-fs"
  cargo test --manifest-path composed-fs/Cargo.toml --offline
  announce "offline tests: vm-frontend"
  cargo test --manifest-path vm-frontend/Cargo.toml --offline
}

fmt_check() {
  announce "rustfmt: composed-fs"
  cargo fmt --manifest-path composed-fs/Cargo.toml --check
  announce "rustfmt: vm-frontend"
  cargo fmt --manifest-path vm-frontend/Cargo.toml --check
  announce "rustfmt: vm-frontend fuzz targets"
  cargo fmt --manifest-path vm-frontend/fuzz/Cargo.toml --check
}

fuzz_check() {
  announce "fuzz target compilation: vm-frontend/fuzz"
  cargo check --manifest-path vm-frontend/fuzz/Cargo.toml --offline
}

guest_services() {
  announce "offline guest service tests"
  ./docker/tests/test_guest_init.sh
  python3 -W error::ResourceWarning -m unittest discover -s docker/tests -p '*test*.py' -v
}

assert_doc_contains() {
  local file="$1"
  local needle="$2"
  if ! grep -Fq "${needle}" "${file}"; then
    cat >&2 <<EOF
error: validation documentation drift detected
  ${file} does not mention: ${needle}

Update AGENTS.md, vm-frontend/validation-workflow.md, and vm-frontend/validate.sh
together when changing the required validation gate.
EOF
    exit 1
  fi
}

docs_check() {
  announce "validation docs drift check"
  assert_doc_contains AGENTS.md "./vm-frontend/validate.sh required"
  assert_doc_contains vm-frontend/validation-workflow.md "vm-frontend/validate.sh required"
  assert_doc_contains vm-frontend/validation-workflow.md "vm-frontend/validate.sh host-live"
}

stress() {
  cargo test --manifest-path composed-fs/Cargo.toml --offline \
    proptest_flat_file_operation_sequences -- --ignored --nocapture
  cargo test --manifest-path composed-fs/Cargo.toml --offline \
    proptest_nested_operation_sequences_stress -- --ignored --nocapture
  cargo test --manifest-path composed-fs/Cargo.toml --offline \
    proptest_lock_operation_sequences_stress -- --ignored --nocapture
  cargo test --manifest-path composed-fs/Cargo.toml --offline \
    stress_seeded_flat_file_operation_sequences -- --ignored --nocapture
  cargo test --manifest-path vm-frontend/Cargo.toml --offline \
    vmnet_stream::tests::stress_many_chunked_frame_splits -- --ignored --nocapture
  cargo test --manifest-path vm-frontend/Cargo.toml --offline \
    vmnet_gateway::tests::stress_seeded_generated_guest_frames -- --ignored --nocapture
  cargo test --manifest-path vm-frontend/Cargo.toml --offline \
    dns_proxy_stress -- --ignored --nocapture
}

require_kvm() {
  local kvm_device="${KVM_DEVICE:-/dev/kvm}"
  if [ ! -e "${kvm_device}" ]; then
    cat >&2 <<EOF
error: host-live limitation: ${kvm_device} is not available.

Live validation boots QEMU with KVM and must run on the host (or a runner with
KVM exposed) after appliance artifacts are built. This is intentionally not a
silent pass; run this tier on a KVM-capable host before closing live validation
work.
EOF
    exit 1
  fi
}

run_self_test_scenario() {
  local name="$1"
  shift
  local run_dir="$1"
  shift
  local qemu="${QEMU:-/usr/bin/qemu-system-x86_64}"
  local image="${IMAGE:-alpine:3.22}"
  announce "${name}: image=${image} qemu=${qemu}"
  cargo run --manifest-path vm-frontend/Cargo.toml --offline -- \
    self-test \
    --project "${ROOT}" \
    --run-dir "${run_dir}" \
    --artifact-manifest "${ROOT}/docker/out/artifact-manifest.json" \
    --qemu "${qemu}" \
    --image "${image}" \
    "$@"
}

live_smoke() {
  require_kvm
  rm -f "${ROOT}/.sandbox/docker-vm/state.raw"
  local publish_port="${PUBLISH_PAYLOAD_PORT:-12079}"
  run_self_test_scenario \
    "live-smoke self-test: publish-payload-port=${publish_port}" \
    "${ROOT}/.sandbox/docker-vm/self-test" \
    --publish-payload-port "${publish_port}"
}

live_hostile() {
  require_kvm
  local saved_image="${IMAGE-}"
  IMAGE="${HOSTILE_IMAGE:-${IMAGE:-alpine:3.22}}"
  run_self_test_scenario \
    "live-hostile self-test: hostile no-net" \
    "${ROOT}/.sandbox/docker-vm/self-test-hostile" \
    --hostile \
    --no-net
  if [ -n "${saved_image}" ]; then
    IMAGE="${saved_image}"
  else
    unset IMAGE
  fi
}

live_payload() {
  require_kvm
  local saved_image="${IMAGE-}"
  IMAGE="${PAYLOAD_IMAGE:-${IMAGE:-alpine:3.22}}"
  run_self_test_scenario \
    "live-payload self-test: large payload request/output" \
    "${ROOT}/.sandbox/docker-vm/self-test-payload" \
    --payload-stress
  if [ -n "${saved_image}" ]; then
    IMAGE="${saved_image}"
  else
    unset IMAGE
  fi
}

live_dns() {
  require_kvm
  run_self_test_scenario \
    "live-dns self-test: allowed resolver path" \
    "${ROOT}/.sandbox/docker-vm/self-test-dns-allow" \
    --dns-check
  run_self_test_scenario \
    "live-dns self-test: denied resolver path" \
    "${ROOT}/.sandbox/docker-vm/self-test-dns-deny" \
    --hostile --no-net
}

live_docker() {
  require_kvm
  rm -f "${ROOT}/.sandbox/docker-vm/state.raw"
  local saved_image="${IMAGE-}"
  IMAGE="${DOCKER_IMAGE:-${IMAGE:-alpine:3.22}}"
  run_self_test_scenario \
    "live-docker self-test: container egress allow" \
    "${ROOT}/.sandbox/docker-vm/self-test-docker-allow" \
    --docker-net-check
  run_self_test_scenario \
    "live-docker self-test: container egress denied no-net" \
    "${ROOT}/.sandbox/docker-vm/self-test-docker-deny" \
    --docker-net-check --no-net
  local publish_host_port="${DOCKER_PUBLISH_HOST_PORT:-12080}"
  local publish_guest_port="${DOCKER_PUBLISH_GUEST_PORT:-18080}"
  run_self_test_scenario \
    "live-docker self-test: host-to-container published port ${publish_host_port}:${publish_guest_port}" \
    "${ROOT}/.sandbox/docker-vm/self-test-docker-publish" \
    --publish-container-port "${publish_host_port}:${publish_guest_port}"
  if [ -n "${saved_image}" ]; then
    IMAGE="${saved_image}"
  else
    unset IMAGE
  fi
}

live_fs() {
  require_kvm
  local run_dir="${ROOT}/.sandbox/docker-vm/self-test-fs"
  run_self_test_scenario \
    "live-fs self-test: composed-fs adversarial first run" \
    "${run_dir}" \
    --fs-check
  run_self_test_scenario \
    "live-fs self-test: composed-fs adversarial repeated run-dir" \
    "${run_dir}" \
    --fs-check
}

live_persistence() {
  require_kvm
  local run_dir="${ROOT}/.sandbox/root-overlay-self-test/run"
  local state_disk="${ROOT}/.sandbox/root-overlay-self-test/state.raw"
  rm -f "${state_disk}"
  run_self_test_scenario \
    "live-persistence self-test: write root overlay marker" \
    "${run_dir}" \
    --root-persistence-check
  run_self_test_scenario \
    "live-persistence self-test: verify root overlay marker after relaunch" \
    "${run_dir}" \
    --root-persistence-check --expect-root-persistence
}

live_full() {
  live_smoke
  live_hostile
  live_payload
  live_dns
  live_docker
  live_fs
  live_persistence
}

required() {
  docs_check
  fmt_check
  fast
  guest_services
  fuzz_check
  live_smoke
}

case "${1:-}" in
  required|full) required ;;
  fast) fast ;;
  fmt) fmt_check ;;
  fuzz-check) fuzz_check ;;
  guest-services) guest_services ;;
  docs) docs_check ;;
  stress) stress ;;
  all-local)
    docs_check
    fmt_check
    fast
    guest_services
    fuzz_check
    stress
    ;;
  live|host-live|live-smoke) live_smoke ;;
  live-hostile) live_hostile ;;
  live-payload) live_payload ;;
  live-dns) live_dns ;;
  live-docker) live_docker ;;
  live-fs) live_fs ;;
  live-persistence) live_persistence ;;
  live-full) live_full ;;
  -h|--help|"")
    usage
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
