#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FRONTEND_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
LOG_DIR="${SCRIPT_DIR}/logs"
RUN_CORPUS_DIR="${SCRIPT_DIR}/run-corpus"

TARGETS=(
  vmnet_stream_frame_io
  dns_proxy_payload
  vmnet_gateway_frame
  composed_manifest_shape
  composed_fs_ops
  composed_fs_locks
)

target=""
run_all=false
run_time=3600
jobs=1
workers=""
rss_mb=4096
timeout=10
repro=""
minimize=""
extra_args=()

usage() {
  cat <<'EOF'
usage: vm-frontend/fuzz/run.sh [options] [-- libfuzzer-args...]

Options:
  --all                  Run all fuzz targets sequentially.
  --target NAME          Run one target. Default: vmnet_gateway_frame.
  --time SECONDS         libFuzzer max_total_time. Default: 3600.
  --jobs N              libFuzzer -jobs. Default: 1.
  --workers N           libFuzzer -workers. Default: same as --jobs.
  --rss-mb N            libFuzzer rss_limit_mb. Default: 4096.
  --timeout SECONDS     libFuzzer per-input timeout. Default: 10.
  --repro PATH          Reproduce a crash/artifact for --target.
  --minimize PATH       Minimize a crash/artifact for --target with cargo fuzz tmin.
  -h, --help            Show this help.

Environment:
  CARGO=cargo           Cargo binary to use.
  FUZZ_TOOLCHAIN=nightly  Cargo toolchain, equivalent to cargo +nightly.
  ASAN_OPTIONS          Defaults to detect_leaks=0:detect_odr_violation=0.
  LSAN_OPTIONS          Defaults to detect_leaks=0.

Examples:
  make fuzz-smoke
  make fuzz-target FUZZ_TARGET=vmnet_gateway_frame FUZZ_TIME=21600 FUZZ_JOBS=8
  make fuzz-extended FUZZ_TIME=43200
  make fuzz-repro FUZZ_TARGET=vmnet_gateway_frame FUZZ_ARTIFACT=vm-frontend/fuzz/artifacts/vmnet_gateway_frame/crash-...

Available targets:
  vmnet_stream_frame_io
  dns_proxy_payload
  vmnet_gateway_frame
  composed_manifest_shape
  composed_fs_ops
  composed_fs_locks
EOF
}

die() {
  echo "error: $*" >&2
  exit 2
}

is_target() {
  local candidate="$1"
  local known
  for known in "${TARGETS[@]}"; do
    if [[ "${known}" == "${candidate}" ]]; then
      return 0
    fi
  done
  return 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --all)
      run_all=true
      shift
      ;;
    --target)
      target="${2:?--target requires a value}"
      shift 2
      ;;
    --time)
      run_time="${2:?--time requires a value}"
      shift 2
      ;;
    --jobs)
      jobs="${2:?--jobs requires a value}"
      shift 2
      ;;
    --workers)
      workers="${2:?--workers requires a value}"
      shift 2
      ;;
    --rss-mb)
      rss_mb="${2:?--rss-mb requires a value}"
      shift 2
      ;;
    --timeout)
      timeout="${2:?--timeout requires a value}"
      shift 2
      ;;
    --repro)
      repro="${2:?--repro requires a value}"
      shift 2
      ;;
    --minimize)
      minimize="${2:?--minimize requires a value}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      extra_args=("$@")
      break
      ;;
    *)
      die "unknown argument $1"
      ;;
  esac
done

if [[ -z "${target}" ]]; then
  target="vmnet_gateway_frame"
fi

if [[ "${run_all}" == true && ( -n "${repro}" || -n "${minimize}" ) ]]; then
  die "--repro and --minimize require a single --target, not --all"
fi

if [[ "${run_all}" == false ]] && ! is_target "${target}"; then
  die "unknown target ${target}"
fi

if [[ -z "${workers}" ]]; then
  workers="${jobs}"
fi

absolute_path() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$(pwd)" "$1" ;;
  esac
}

if [[ -n "${repro}" ]]; then
  repro="$(absolute_path "${repro}")"
fi

if [[ -n "${minimize}" ]]; then
  minimize="$(absolute_path "${minimize}")"
fi

cargo_bin="${CARGO:-cargo}"
cargo_args=()
FUZZ_TOOLCHAIN="${FUZZ_TOOLCHAIN:-nightly}"
if [[ -n "${FUZZ_TOOLCHAIN}" ]]; then
  cargo_args+=("+${FUZZ_TOOLCHAIN}")
fi

if ! command -v "${cargo_bin}" >/dev/null 2>&1; then
  die "cargo binary not found: ${cargo_bin}"
fi

if ! "${cargo_bin}" "${cargo_args[@]}" fuzz --help >/dev/null 2>&1; then
  cat >&2 <<'EOF'
error: cargo-fuzz is not installed or is unavailable for this toolchain.

Install it with:
  cargo install cargo-fuzz

If nightly is not installed, install it with:
  rustup toolchain install nightly
EOF
  exit 2
fi

export ASAN_OPTIONS="${ASAN_OPTIONS:-detect_leaks=0:detect_odr_violation=0}"
export LSAN_OPTIONS="${LSAN_OPTIONS:-detect_leaks=0}"

mkdir -p "${LOG_DIR}"
cd "${FRONTEND_DIR}"

seed_run_corpus() {
  local source="$1"
  local destination="$2"
  local seed
  local basename

  [[ -d "${source}" ]] || return 0

  while IFS= read -r -d '' seed; do
    basename="${seed##*/}"
    if [[ "${basename}" =~ ^[0-9a-f]{40}$ ]]; then
      continue
    fi
    if [[ ! -e "${destination}/${basename}" ]]; then
      cp "${seed}" "${destination}/${basename}"
    fi
  done < <(find "${source}" -maxdepth 1 -type f -print0)
}

run_target() {
  local name="$1"
  local stamp
  stamp="$(date -u +%Y%m%dT%H%M%SZ)"
  local log="${LOG_DIR}/${name}-${stamp}.log"
  local run_corpus="${RUN_CORPUS_DIR}/${name}"
  local seed_corpus="${SCRIPT_DIR}/corpus/${name}"

  mkdir -p "${run_corpus}"
  seed_run_corpus "${seed_corpus}" "${run_corpus}"

  echo "==> fuzz target ${name}; mutable corpus ${run_corpus}; log ${log}"
  "${cargo_bin}" "${cargo_args[@]}" fuzz run "${name}" "${run_corpus}" -- \
    -max_total_time="${run_time}" \
    -rss_limit_mb="${rss_mb}" \
    -timeout="${timeout}" \
    -jobs="${jobs}" \
    -workers="${workers}" \
    "${extra_args[@]}" 2>&1 | tee "${log}"
}

if [[ -n "${minimize}" ]]; then
  echo "==> minimizing ${minimize} for target ${target}"
  exec "${cargo_bin}" "${cargo_args[@]}" fuzz tmin "${target}" "${minimize}"
fi

if [[ -n "${repro}" ]]; then
  echo "==> reproducing ${repro} for target ${target}"
  exec "${cargo_bin}" "${cargo_args[@]}" fuzz run "${target}" "${repro}"
fi

if [[ "${run_all}" == true ]]; then
  for name in "${TARGETS[@]}"; do
    run_target "${name}"
  done
else
  run_target "${target}"
fi
