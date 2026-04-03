#!/usr/bin/env bash

# Shared release metadata keeps packaging scripts aligned.
AADK_RELEASE_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AADK_RELEASE_ROOT="$(cd "${AADK_RELEASE_SCRIPT_DIR}/../.." && pwd)"

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
    install -m 755 "${AADK_RELEASE_ROOT}/target/release/${bin}" "${dest_dir}/${bin}"
  done
}

aadk_release_install_docs() {
  local dest_dir="$1"

  install -d "$dest_dir"
  install -m 644 "${AADK_RELEASE_ROOT}/README.md" "${dest_dir}/README.md"
  install -m 644 "${AADK_RELEASE_ROOT}/LICENSE" "${dest_dir}/LICENSE"
}
