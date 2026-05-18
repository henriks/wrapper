#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
readonly ENV_PATH="${SCRIPT_DIR}/appliance.env"

source "${ENV_PATH}"

require_commands() {
  local missing=()
  local cmd
  for cmd in awk curl mktemp tar python3; do
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
  local docker_cli_version=$2
  local linux_virt_version=$3
  local mkinitfs_version=$4
  local python3_version=$5
  local e2fsprogs_version=$6
  local iproute2_version=$7
  local util_linux_version=$8
  local bash_version=$9
  local nodejs_version=${10}
  local npm_version=${11}
  local mise_version=${12}
  local mise_sha256=${13}
  local tmp

  tmp=$(mktemp)
  awk \
    -v docker_engine_version="${docker_engine_version}" \
    -v docker_cli_version="${docker_cli_version}" \
    -v linux_virt_version="${linux_virt_version}" \
    -v mkinitfs_version="${mkinitfs_version}" \
    -v python3_version="${python3_version}" \
    -v e2fsprogs_version="${e2fsprogs_version}" \
    -v iproute2_version="${iproute2_version}" \
    -v util_linux_version="${util_linux_version}" \
    -v bash_version="${bash_version}" \
    -v nodejs_version="${nodejs_version}" \
    -v npm_version="${npm_version}" \
    -v mise_version="${mise_version}" \
    -v mise_sha256="${mise_sha256}" '
      /^DOCKER_ENGINE_VERSION=/ { print "DOCKER_ENGINE_VERSION=" docker_engine_version; next }
      /^DOCKER_CLI_VERSION=/ { print "DOCKER_CLI_VERSION=" docker_cli_version; next }
      /^LINUX_VIRT_VERSION=/ { print "LINUX_VIRT_VERSION=" linux_virt_version; next }
      /^MKINITFS_VERSION=/ { print "MKINITFS_VERSION=" mkinitfs_version; next }
      /^PYTHON3_VERSION=/ { print "PYTHON3_VERSION=" python3_version; next }
      /^E2FSPROGS_VERSION=/ { print "E2FSPROGS_VERSION=" e2fsprogs_version; next }
      /^IPROUTE2_VERSION=/ { print "IPROUTE2_VERSION=" iproute2_version; next }
      /^UTIL_LINUX_VERSION=/ { print "UTIL_LINUX_VERSION=" util_linux_version; next }
      /^BASH_VERSION=/ { print "BASH_VERSION=" bash_version; next }
      /^NODEJS_VERSION=/ { print "NODEJS_VERSION=" nodejs_version; next }
      /^NPM_VERSION=/ { print "NPM_VERSION=" npm_version; next }
      /^MISE_VERSION=/ { print "MISE_VERSION=" mise_version; next }
      /^MISE_SHA256=/ { print "MISE_SHA256=" mise_sha256; next }
      { print }
    ' "${ENV_PATH}" > "${tmp}"
  mv "${tmp}" "${ENV_PATH}"
}

resolve_mise_release() {
  local json_path=$1
  local target=$2

  python3 - "$json_path" "$target" <<'PY'
import json
import sys

json_path, target = sys.argv[1], sys.argv[2]
with open(json_path, "r", encoding="utf-8") as fh:
    data = json.load(fh)

tag = data.get("tag_name", "")
asset_name = f"mise-{tag}-{target}"

for asset in data.get("assets", []):
    if asset.get("name") != asset_name:
        continue
    digest = asset.get("digest", "")
    if digest.startswith("sha256:"):
        print(tag)
        print(digest.split(":", 1)[1])
        sys.exit(0)

sys.exit(1)
PY
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
  local docker_cli_version
  local linux_virt_version
  local mkinitfs_version
  local python3_version
  local e2fsprogs_version
  local iproute2_version
  local util_linux_version
  local bash_version
  local nodejs_version
  local npm_version
  local mise_version
  local mise_sha256
  local mise_release_json
  local mise_release_info

  docker_engine_version=$(resolve_version docker-engine "${community_index}" "${main_index}")
  docker_cli_version=$(resolve_version docker-cli "${community_index}" "${main_index}")
  linux_virt_version=$(resolve_version linux-virt "${main_index}" "${community_index}")
  mkinitfs_version=$(resolve_version mkinitfs "${main_index}" "${community_index}")
  python3_version=$(resolve_version python3 "${main_index}" "${community_index}")
  e2fsprogs_version=$(resolve_version e2fsprogs "${main_index}" "${community_index}")
  iproute2_version=$(resolve_version iproute2 "${main_index}" "${community_index}")
  util_linux_version=$(resolve_version util-linux "${main_index}" "${community_index}")
  bash_version=$(resolve_version bash "${main_index}" "${community_index}")
  nodejs_version=$(resolve_version nodejs "${main_index}" "${community_index}")
  npm_version=$(resolve_version npm "${main_index}" "${community_index}")
  mise_release_json=$(mktemp)
  trap 'rm -f "${main_index:-}" "${community_index:-}" "${mise_release_json:-}"' EXIT
  curl -fsSL "https://api.github.com/repos/jdx/mise/releases/latest" -o "${mise_release_json}"
  mise_release_info=$(resolve_mise_release "${mise_release_json}" "${MISE_TARGET}") || {
    echo "error: failed to resolve mise release metadata for target ${MISE_TARGET}" >&2
    exit 1
  }
  mise_version=$(printf '%s\n' "${mise_release_info}" | sed -n '1p')
  mise_sha256=$(printf '%s\n' "${mise_release_info}" | sed -n '2p')

  if [[ -z "${docker_engine_version}" || -z "${docker_cli_version}" || -z "${linux_virt_version}" || -z "${mkinitfs_version}" || -z "${python3_version}" || -z "${e2fsprogs_version}" || -z "${iproute2_version}" || -z "${util_linux_version}" || -z "${bash_version}" || -z "${nodejs_version}" || -z "${npm_version}" || -z "${mise_version}" || -z "${mise_sha256}" ]]; then
    echo "error: failed to resolve one or more Alpine package versions" >&2
    exit 1
  fi

  rewrite_env_file \
    "${docker_engine_version}" \
    "${docker_cli_version}" \
    "${linux_virt_version}" \
    "${mkinitfs_version}" \
    "${python3_version}" \
    "${e2fsprogs_version}" \
    "${iproute2_version}" \
    "${util_linux_version}" \
    "${bash_version}" \
    "${nodejs_version}" \
    "${npm_version}" \
    "${mise_version}" \
    "${mise_sha256}"

  cat <<EOF
Updated ${ENV_PATH}
  DOCKER_ENGINE_VERSION=${docker_engine_version}
  DOCKER_CLI_VERSION=${docker_cli_version}
  LINUX_VIRT_VERSION=${linux_virt_version}
  MKINITFS_VERSION=${mkinitfs_version}
  PYTHON3_VERSION=${python3_version}
  E2FSPROGS_VERSION=${e2fsprogs_version}
  IPROUTE2_VERSION=${iproute2_version}
  UTIL_LINUX_VERSION=${util_linux_version}
  BASH_VERSION=${bash_version}
  NODEJS_VERSION=${nodejs_version}
  NPM_VERSION=${npm_version}
  MISE_VERSION=${mise_version}
  MISE_SHA256=${mise_sha256}
EOF
}

main "$@"
