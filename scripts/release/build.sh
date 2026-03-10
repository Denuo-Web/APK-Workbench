#!/usr/bin/env bash
set -euo pipefail

VERSION="${VERSION:-0.1.0}"
OUT="dist/aadk-${VERSION}-linux-aarch64"
ARCHIVE="dist/aadk-${VERSION}-linux-aarch64.tar.gz"
CHECKSUM="${ARCHIVE}.sha256"

cargo build --release --workspace
ls -1 target/release/aadk-*

mkdir -p "${OUT}"
cp target/release/aadk-{core,workflow,toolchain,project,build,targets,observe,ui,cli} "${OUT}/"
cp scripts/release/aadk-start.sh "${OUT}/aadk-start.sh"
cp README.md LICENSE "${OUT}/"
tar -C dist -czf "${ARCHIVE}" "aadk-${VERSION}-linux-aarch64"
sha256sum "${ARCHIVE}" > "${CHECKSUM}"
