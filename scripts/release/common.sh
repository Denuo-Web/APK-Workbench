#!/usr/bin/env bash

# Shared release metadata keeps packaging scripts aligned.
APKW_RELEASE_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APKW_RELEASE_ROOT="$(cd "${APKW_RELEASE_SCRIPT_DIR}/../.." && pwd)"
# shellcheck source=scripts/release/apkw-env.sh
source "${APKW_RELEASE_SCRIPT_DIR}/apkw-env.sh"

APKW_RELEASE_DEFAULT_PKGNAME="apkw"
APKW_RELEASE_DEB_MAINTAINER="${APKW_RELEASE_DEB_MAINTAINER:-Jaron Rosenau <jaron@rosenau.info>}"
APKW_RELEASE_DEB_SUMMARY="${APKW_RELEASE_DEB_SUMMARY:-APK Workbench workflow suite}"
APKW_RELEASE_DEB_LIBDIR="/usr/lib/apkw"

apkw_release_binaries=(
  apkw-core
  apkw-workflow
  apkw-toolchain
  apkw-project
  apkw-build
  apkw-targets
  apkw-observe
  apkw-ui
  apkw-cli
)

apkw_workspace_version() {
  local version

  version="$(
    awk '
      /^\[workspace\.package\]/ { in_workspace_package = 1; next }
      /^\[/ { in_workspace_package = 0 }
      in_workspace_package && /^version = "/ {
        line = $0
        sub(/^version = "/, "", line)
        sub(/"$/, "", line)
        print line
        exit
      }
    ' "${APKW_RELEASE_ROOT}/Cargo.toml"
  )"
  if [ -z "$version" ]; then
    echo "ERROR: failed to read [workspace.package] version from Cargo.toml" >&2
    return 1
  fi
  printf '%s\n' "$version"
}

apkw_release_default_pkgname() {
  printf '%s\n' "$APKW_RELEASE_DEFAULT_PKGNAME"
}

apkw_release_validate_deb_pkgname() {
  local pkgname="$1"

  if ! command -v dpkg >/dev/null 2>&1; then
    echo "ERROR: dpkg not found. Install dpkg before building Debian packages." >&2
    exit 1
  fi

  if ! dpkg --validate-pkgname "$pkgname" >/dev/null 2>&1; then
    echo "ERROR: invalid Debian package name: ${pkgname}" >&2
    exit 1
  fi

  if ! printf '%s\n' "$pkgname" | grep -Eq '^[a-z0-9][a-z0-9.+-]*$'; then
    echo "ERROR: invalid Debian package name: ${pkgname}" >&2
    echo "Use lowercase letters, digits, '+', '-', or '.', and start with a letter or digit." >&2
    exit 1
  fi
}

apkw_release_escape_sed_replacement() {
  printf '%s' "$1" | sed -e 's/[\\/&]/\\&/g'
}

apkw_release_rfc2822_date() {
  if [ -n "${SOURCE_DATE_EPOCH:-}" ]; then
    date -u -d "@${SOURCE_DATE_EPOCH}" -R
    return 0
  fi

  date -R
}

apkw_release_require_linux_arm64() {
  local host_os host_arch

  if [ "${APKW_ALLOW_UNSUPPORTED_RELEASE_HOST:-0}" = "1" ]; then
    return 0
  fi

  host_os="$(uname -s)"
  host_arch="$(uname -m)"
  if [ "$host_os" != "Linux" ]; then
    echo "ERROR: release packaging is only supported on Linux ARM64 hosts." >&2
    echo "Set APKW_ALLOW_UNSUPPORTED_RELEASE_HOST=1 only for explicit experimental packaging." >&2
    exit 1
  fi
  case "$host_arch" in
    aarch64 | arm64)
      ;;
    *)
      echo "ERROR: release packaging is only supported on Linux ARM64 hosts; found ${host_arch}." >&2
      echo "Set APKW_ALLOW_UNSUPPORTED_RELEASE_HOST=1 only for explicit experimental packaging." >&2
      exit 1
      ;;
  esac
}

apkw_release_require_debian_arch() {
  local expected_arch="$1"
  local host_arch

  if [ "${APKW_ALLOW_UNSUPPORTED_RELEASE_HOST:-0}" = "1" ]; then
    return 0
  fi

  if [ "$expected_arch" != "arm64" ]; then
    echo "ERROR: Debian packaging only supports ARCH=arm64 by default; found ${expected_arch}." >&2
    echo "Set APKW_ALLOW_UNSUPPORTED_RELEASE_HOST=1 only for explicit experimental packaging." >&2
    exit 1
  fi

  if command -v dpkg >/dev/null 2>&1; then
    host_arch="$(dpkg --print-architecture)"
    if [ "$host_arch" != "$expected_arch" ]; then
      echo "ERROR: Debian host architecture ${host_arch} does not match ARCH=${expected_arch}." >&2
      echo "Set APKW_ALLOW_UNSUPPORTED_RELEASE_HOST=1 only for explicit experimental packaging." >&2
      exit 1
    fi
  fi
}

apkw_release_build_workspace() {
  cargo build --release --workspace --locked
}

apkw_release_print_binaries() {
  local bin

  for bin in "${apkw_release_binaries[@]}"; do
    printf 'target/release/%s\n' "$bin"
  done
}

apkw_release_install_binaries() {
  local dest_dir="$1"
  local bin

  install -d "$dest_dir"
  for bin in "${apkw_release_binaries[@]}"; do
    if [ ! -x "${APKW_RELEASE_ROOT}/target/release/${bin}" ]; then
      echo "ERROR: missing built binary target/release/${bin}" >&2
      exit 1
    fi
    install -m 755 -s "${APKW_RELEASE_ROOT}/target/release/${bin}" "${dest_dir}/${bin}"
  done
}

apkw_release_install_launcher() {
  local dest_dir="$1"
  local launcher_name="${2:-apkw-start.sh}"

  install -d "$dest_dir"
  install -m 755 "${APKW_RELEASE_ROOT}/scripts/release/apkw-start.sh" "${dest_dir}/${launcher_name}"
  install -m 755 "${APKW_RELEASE_ROOT}/scripts/release/apkw-env.sh" "${dest_dir}/apkw-env.sh"
}

apkw_release_install_docs() {
  local dest_dir="$1"

  install -d "$dest_dir"
  install -m 644 "${APKW_RELEASE_ROOT}/README.md" "${dest_dir}/README.md"
  install -m 644 "${APKW_RELEASE_ROOT}/LICENSE" "${dest_dir}/LICENSE"
}
