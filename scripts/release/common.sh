#!/usr/bin/env bash

# Shared release metadata keeps packaging scripts aligned.
AADK_RELEASE_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AADK_RELEASE_ROOT="$(cd "${AADK_RELEASE_SCRIPT_DIR}/../.." && pwd)"
# shellcheck source=scripts/release/aadk-env.sh
source "${AADK_RELEASE_SCRIPT_DIR}/aadk-env.sh"

AADK_RELEASE_DEFAULT_PKGNAME="aadk"
AADK_RELEASE_DEB_MAINTAINER="${AADK_RELEASE_DEB_MAINTAINER:-Jaron Rosenau <jaron@rosenau.info>}"
AADK_RELEASE_DEB_SUMMARY="${AADK_RELEASE_DEB_SUMMARY:-Android DevKit workflow suite}"
AADK_RELEASE_DEB_LIBDIR="/usr/lib/aadk"

aadk_release_binaries=(
  aadk-core
  aadk-workflow
  aadk-toolchain
  aadk-project
  aadk-build
  aadk-targets
  aadk-observe
  aadk-ui
  aadk-cli
)

aadk_workspace_version() {
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
    ' "${AADK_RELEASE_ROOT}/Cargo.toml"
  )"
  if [ -z "$version" ]; then
    echo "ERROR: failed to read [workspace.package] version from Cargo.toml" >&2
    return 1
  fi
  printf '%s\n' "$version"
}

aadk_release_default_pkgname() {
  printf '%s\n' "$AADK_RELEASE_DEFAULT_PKGNAME"
}

aadk_release_validate_deb_pkgname() {
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

aadk_release_escape_sed_replacement() {
  printf '%s' "$1" | sed -e 's/[\\/&]/\\&/g'
}

aadk_release_rfc2822_date() {
  if [ -n "${SOURCE_DATE_EPOCH:-}" ]; then
    date -u -d "@${SOURCE_DATE_EPOCH}" -R
    return 0
  fi

  date -R
}

aadk_release_require_linux_arm64() {
  local host_os host_arch

  if [ "${AADK_ALLOW_UNSUPPORTED_RELEASE_HOST:-0}" = "1" ]; then
    return 0
  fi

  host_os="$(uname -s)"
  host_arch="$(uname -m)"
  if [ "$host_os" != "Linux" ]; then
    echo "ERROR: release packaging is only supported on Linux ARM64 hosts." >&2
    echo "Set AADK_ALLOW_UNSUPPORTED_RELEASE_HOST=1 only for explicit experimental packaging." >&2
    exit 1
  fi
  case "$host_arch" in
    aarch64 | arm64)
      ;;
    *)
      echo "ERROR: release packaging is only supported on Linux ARM64 hosts; found ${host_arch}." >&2
      echo "Set AADK_ALLOW_UNSUPPORTED_RELEASE_HOST=1 only for explicit experimental packaging." >&2
      exit 1
      ;;
  esac
}

aadk_release_require_debian_arch() {
  local expected_arch="$1"
  local host_arch

  if [ "${AADK_ALLOW_UNSUPPORTED_RELEASE_HOST:-0}" = "1" ]; then
    return 0
  fi

  if [ "$expected_arch" != "arm64" ]; then
    echo "ERROR: Debian packaging only supports ARCH=arm64 by default; found ${expected_arch}." >&2
    echo "Set AADK_ALLOW_UNSUPPORTED_RELEASE_HOST=1 only for explicit experimental packaging." >&2
    exit 1
  fi

  if command -v dpkg >/dev/null 2>&1; then
    host_arch="$(dpkg --print-architecture)"
    if [ "$host_arch" != "$expected_arch" ]; then
      echo "ERROR: Debian host architecture ${host_arch} does not match ARCH=${expected_arch}." >&2
      echo "Set AADK_ALLOW_UNSUPPORTED_RELEASE_HOST=1 only for explicit experimental packaging." >&2
      exit 1
    fi
  fi
}

aadk_release_build_workspace() {
  cargo build --release --workspace --locked
}

aadk_release_print_binaries() {
  local bin

  for bin in "${aadk_release_binaries[@]}"; do
    printf 'target/release/%s\n' "$bin"
  done
}

aadk_release_install_binaries() {
  local dest_dir="$1"
  local bin

  install -d "$dest_dir"
  for bin in "${aadk_release_binaries[@]}"; do
    if [ ! -x "${AADK_RELEASE_ROOT}/target/release/${bin}" ]; then
      echo "ERROR: missing built binary target/release/${bin}" >&2
      exit 1
    fi
    install -m 755 -s "${AADK_RELEASE_ROOT}/target/release/${bin}" "${dest_dir}/${bin}"
  done
}

aadk_release_install_launcher() {
  local dest_dir="$1"
  local launcher_name="${2:-aadk-start.sh}"

  install -d "$dest_dir"
  install -m 755 "${AADK_RELEASE_ROOT}/scripts/release/aadk-start.sh" "${dest_dir}/${launcher_name}"
  install -m 755 "${AADK_RELEASE_ROOT}/scripts/release/aadk-env.sh" "${dest_dir}/aadk-env.sh"
}

aadk_release_install_docs() {
  local dest_dir="$1"

  install -d "$dest_dir"
  install -m 644 "${AADK_RELEASE_ROOT}/README.md" "${dest_dir}/README.md"
  install -m 644 "${AADK_RELEASE_ROOT}/LICENSE" "${dest_dir}/LICENSE"
}
