#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
readonly ENV_PATH="${SCRIPT_DIR}/appliance.env"

source "${ENV_PATH}"

require_commands() {
  local missing=()
  local cmd
  for cmd in awk curl mktemp tar; do
    if ! command -v "${cmd}" >/dev/null 2>&1; then
      missing+=("${cmd}")
    fi
  done
  if ((${#missing[@]} > 0)); then
    printf 'error: missing required commands: %s\n' "${missing[*]}" >&2
    exit 1
  fi
}

apkindex_url() {
  local repo=$1
  printf '%s/%s/%s/%s/APKINDEX.tar.gz\n' \
    "${ALPINE_MIRROR}" \
    "${ALPINE_BRANCH}" \
    "${repo}" \
    "${ALPINE_ARCH}"
}

fetch_apkindex() {
  local repo=$1
  local tmp
  tmp=$(mktemp)
  curl -fsSL "$(apkindex_url "${repo}")" | tar -xzOf - APKINDEX > "${tmp}"
  printf '%s\n' "${tmp}"
}

resolve_version() {
  local package_name=$1
  shift

  local index_file
  for index_file in "$@"; do
    awk -v pkg="${package_name}" '
      BEGIN { RS=""; FS="\n" }
      {
        package=""
        version=""
        for (i = 1; i <= NF; i++) {
          if ($i ~ /^P:/) {
            package = substr($i, 3)
          } else if ($i ~ /^V:/) {
            version = substr($i, 3)
          }
        }
        if (package == pkg) {
          print version
          exit
        }
      }
    ' "${index_file}" | head -n 1
  done | head -n 1
}

rewrite_env_file() {
  local docker_engine_version=$1
  local linux_virt_version=$2
  local mkinitfs_version=$3
  local python3_version=$4
  local e2fsprogs_version=$5
  local iproute2_version=$6
  local util_linux_version=$7
  local tmp

  tmp=$(mktemp)
  awk \
    -v docker_engine_version="${docker_engine_version}" \
    -v linux_virt_version="${linux_virt_version}" \
    -v mkinitfs_version="${mkinitfs_version}" \
    -v python3_version="${python3_version}" \
    -v e2fsprogs_version="${e2fsprogs_version}" \
    -v iproute2_version="${iproute2_version}" \
    -v util_linux_version="${util_linux_version}" '
      /^DOCKER_ENGINE_VERSION=/ { print "DOCKER_ENGINE_VERSION=" docker_engine_version; next }
      /^LINUX_VIRT_VERSION=/ { print "LINUX_VIRT_VERSION=" linux_virt_version; next }
      /^MKINITFS_VERSION=/ { print "MKINITFS_VERSION=" mkinitfs_version; next }
      /^PYTHON3_VERSION=/ { print "PYTHON3_VERSION=" python3_version; next }
      /^E2FSPROGS_VERSION=/ { print "E2FSPROGS_VERSION=" e2fsprogs_version; next }
      /^IPROUTE2_VERSION=/ { print "IPROUTE2_VERSION=" iproute2_version; next }
      /^UTIL_LINUX_VERSION=/ { print "UTIL_LINUX_VERSION=" util_linux_version; next }
      { print }
    ' "${ENV_PATH}" > "${tmp}"
  mv "${tmp}" "${ENV_PATH}"
}

main() {
  require_commands

  local main_index community_index
  main_index=""
  community_index=""
  trap 'rm -f "${main_index:-}" "${community_index:-}"' EXIT
  main_index=$(fetch_apkindex main)
  community_index=$(fetch_apkindex community)

  local docker_engine_version
  local linux_virt_version
  local mkinitfs_version
  local python3_version
  local e2fsprogs_version
  local iproute2_version
  local util_linux_version

  docker_engine_version=$(resolve_version docker-engine "${community_index}" "${main_index}")
  linux_virt_version=$(resolve_version linux-virt "${main_index}" "${community_index}")
  mkinitfs_version=$(resolve_version mkinitfs "${main_index}" "${community_index}")
  python3_version=$(resolve_version python3 "${main_index}" "${community_index}")
  e2fsprogs_version=$(resolve_version e2fsprogs "${main_index}" "${community_index}")
  iproute2_version=$(resolve_version iproute2 "${main_index}" "${community_index}")
  util_linux_version=$(resolve_version util-linux "${main_index}" "${community_index}")

  if [[ -z "${docker_engine_version}" || -z "${linux_virt_version}" || -z "${mkinitfs_version}" || -z "${python3_version}" || -z "${e2fsprogs_version}" || -z "${iproute2_version}" || -z "${util_linux_version}" ]]; then
    echo "error: failed to resolve one or more Alpine package versions" >&2
    exit 1
  fi

  rewrite_env_file \
    "${docker_engine_version}" \
    "${linux_virt_version}" \
    "${mkinitfs_version}" \
    "${python3_version}" \
    "${e2fsprogs_version}" \
    "${iproute2_version}" \
    "${util_linux_version}"

  cat <<EOF
Updated ${ENV_PATH}
  DOCKER_ENGINE_VERSION=${docker_engine_version}
  LINUX_VIRT_VERSION=${linux_virt_version}
  MKINITFS_VERSION=${mkinitfs_version}
  PYTHON3_VERSION=${python3_version}
  E2FSPROGS_VERSION=${e2fsprogs_version}
  IPROUTE2_VERSION=${iproute2_version}
  UTIL_LINUX_VERSION=${util_linux_version}
EOF
}

main "$@"
