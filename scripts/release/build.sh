#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/release/common.sh
source "${SCRIPT_DIR}/common.sh"

cd "${APKW_RELEASE_ROOT}"

VERSION="${VERSION:-$(apkw_workspace_version)}"
OUT="dist/apkw-${VERSION}-linux-aarch64"
ARCHIVE="dist/apkw-${VERSION}-linux-aarch64.tar.gz"
CHECKSUM="${ARCHIVE}.sha256"
ARCHIVE_NAME="$(basename "${ARCHIVE}")"
CHECKSUM_NAME="$(basename "${CHECKSUM}")"

apkw_release_require_linux_arm64
mkdir -p dist

apkw_release_build_workspace
apkw_release_print_binaries

rm -rf "${OUT}"
apkw_release_install_binaries "${OUT}"
apkw_release_install_launcher "${OUT}" "apkw-start.sh"
apkw_release_install_docs "${OUT}"
rm -f "${ARCHIVE}" "${CHECKSUM}"
tar -C dist -czf "${ARCHIVE}" "apkw-${VERSION}-linux-aarch64"
(
  cd dist
  sha256sum "${ARCHIVE_NAME}" > "${CHECKSUM_NAME}"
)

echo "Built ${ARCHIVE}"
echo "Built ${CHECKSUM}"
