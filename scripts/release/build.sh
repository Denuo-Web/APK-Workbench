#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/release/common.sh
source "${SCRIPT_DIR}/common.sh"

cd "${AADK_RELEASE_ROOT}"

VERSION="${VERSION:-$(aadk_workspace_version)}"
OUT="dist/aadk-${VERSION}-linux-aarch64"
ARCHIVE="dist/aadk-${VERSION}-linux-aarch64.tar.gz"
CHECKSUM="${ARCHIVE}.sha256"

aadk_release_require_linux_arm64
mkdir -p dist

aadk_release_build_workspace
aadk_release_print_binaries

rm -rf "${OUT}"
aadk_release_install_binaries "${OUT}"
aadk_release_install_launcher "${OUT}" "aadk-start.sh"
aadk_release_install_docs "${OUT}"
rm -f "${ARCHIVE}" "${CHECKSUM}"
tar -C dist -czf "${ARCHIVE}" "aadk-${VERSION}-linux-aarch64"
sha256sum "${ARCHIVE}" > "${CHECKSUM}"

echo "Built ${ARCHIVE}"
echo "Built ${CHECKSUM}"
