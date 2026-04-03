#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/release/common.sh
source "${SCRIPT_DIR}/common.sh"

cd "${AADK_RELEASE_ROOT}"

VERSION="${VERSION:-$(aadk_workspace_version)}"
ARCH="${ARCH:-arm64}"
PKGNAME="${PKGNAME:-$(aadk_release_default_pkgname)}"
DEB_MAINTAINER="${DEB_MAINTAINER:-$AADK_RELEASE_DEB_MAINTAINER}"
DEB_SUMMARY="${DEB_SUMMARY:-$AADK_RELEASE_DEB_SUMMARY}"
JAVA_RUNTIME_RECOMMENDS="${JAVA_RUNTIME_RECOMMENDS:-$(aadk_supported_java_jre_recommends)}"
CHANGELOG_DATE="${CHANGELOG_DATE:-$(aadk_release_rfc2822_date)}"

aadk_release_validate_deb_pkgname "${PKGNAME}"

ROOT="dist/deb/${PKGNAME}_${VERSION}_${ARCH}"
INSTALL_ROOT="$ROOT${AADK_RELEASE_DEB_LIBDIR}"
BIN_SYMLINK_ROOT="../${AADK_RELEASE_DEB_LIBDIR#/usr/}/bin"
BIN_DIR="$INSTALL_ROOT/bin"
DOC_DIR="$ROOT/usr/share/doc/${PKGNAME}"
APP_DIR="$ROOT/usr/share/applications"
ICON_DIR="$ROOT/usr/share/icons/hicolor/scalable/apps"
MAN_DIR="$ROOT/usr/share/man/man1"
DEBIAN_DIR="$ROOT/DEBIAN"
OUTPUT="dist/${PKGNAME}_${VERSION}_${ARCH}.deb"

if ! command -v dpkg-deb >/dev/null 2>&1; then
  echo "ERROR: dpkg-deb not found. Install dpkg-dev." >&2
  exit 1
fi

aadk_release_require_linux_arm64
aadk_release_require_debian_arch "${ARCH}"

if [ ! -f "scripts/release/aadk-start.sh" ]; then
  echo "ERROR: missing scripts/release/aadk-start.sh" >&2
  exit 1
fi

if [ ! -f "scripts/release/aadk-env.sh" ]; then
  echo "ERROR: missing scripts/release/aadk-env.sh" >&2
  exit 1
fi

if [ ! -f "packaging/deb/control.in" ]; then
  echo "ERROR: missing packaging/deb/control.in" >&2
  exit 1
fi

if [ ! -f "packaging/deb/changelog.in" ]; then
  echo "ERROR: missing packaging/deb/changelog.in" >&2
  exit 1
fi

if [ ! -f "packaging/deb/aadk.desktop" ]; then
  echo "ERROR: missing packaging/deb/aadk.desktop" >&2
  exit 1
fi

if [ ! -f "assets/aadk.svg" ]; then
  echo "ERROR: missing assets/aadk.svg" >&2
  exit 1
fi

for manpage in aadk aadk-ui aadk-cli; do
  if [ ! -f "packaging/deb/man/${manpage}.1" ]; then
    echo "ERROR: missing packaging/deb/man/${manpage}.1" >&2
    exit 1
  fi
done

mkdir -p dist

aadk_release_build_workspace
aadk_release_print_binaries

rm -rf "$ROOT"
install -d "$BIN_DIR" "$DOC_DIR" "$APP_DIR" "$ICON_DIR" "$MAN_DIR" "$DEBIAN_DIR" "$ROOT/usr/bin" "$ROOT/usr/lib"

aadk_release_install_binaries "$BIN_DIR"
aadk_release_install_launcher "$BIN_DIR" "aadk-start"

aadk_release_install_docs "$DOC_DIR"
install -m 644 LICENSE "$DOC_DIR/copyright"
install -m 644 packaging/deb/aadk.desktop "$APP_DIR/aadk.desktop"
install -m 644 assets/aadk.svg "$ICON_DIR/aadk.svg"
for manpage in aadk aadk-ui aadk-cli; do
  gzip -9nc "packaging/deb/man/${manpage}.1" > "$MAN_DIR/${manpage}.1.gz"
  chmod 644 "$MAN_DIR/${manpage}.1.gz"
done

ln -s "${BIN_SYMLINK_ROOT}/aadk-start" "$ROOT/usr/bin/aadk"
ln -s "${BIN_SYMLINK_ROOT}/aadk-ui" "$ROOT/usr/bin/aadk-ui"
ln -s "${BIN_SYMLINK_ROOT}/aadk-cli" "$ROOT/usr/bin/aadk-cli"

pkgname_escaped="$(aadk_release_escape_sed_replacement "$PKGNAME")"
version_escaped="$(aadk_release_escape_sed_replacement "$VERSION")"
arch_escaped="$(aadk_release_escape_sed_replacement "$ARCH")"
maintainer_escaped="$(aadk_release_escape_sed_replacement "$DEB_MAINTAINER")"
summary_escaped="$(aadk_release_escape_sed_replacement "$DEB_SUMMARY")"
java_runtime_recommends_escaped="$(aadk_release_escape_sed_replacement "$JAVA_RUNTIME_RECOMMENDS")"
changelog_date_escaped="$(aadk_release_escape_sed_replacement "$CHANGELOG_DATE")"

sed \
  -e "s/@PKGNAME@/${pkgname_escaped}/g" \
  -e "s/@VERSION@/${version_escaped}/g" \
  -e "s/@ARCH@/${arch_escaped}/g" \
  -e "s/@MAINTAINER@/${maintainer_escaped}/g" \
  -e "s/@SUMMARY@/${summary_escaped}/g" \
  -e "s/@JAVA_RUNTIME_RECOMMENDS@/${java_runtime_recommends_escaped}/g" \
  packaging/deb/control.in > "$DEBIAN_DIR/control"
sed \
  -e "s/@PKGNAME@/${pkgname_escaped}/g" \
  -e "s/@VERSION@/${version_escaped}/g" \
  -e "s/@MAINTAINER@/${maintainer_escaped}/g" \
  -e "s/@DATE_RFC2822@/${changelog_date_escaped}/g" \
  packaging/deb/changelog.in | gzip -9n > "$DOC_DIR/changelog.gz"
chmod 644 "$DOC_DIR/changelog.gz"
install -m 755 packaging/deb/postinst "$DEBIAN_DIR/postinst"
install -m 755 packaging/deb/postrm "$DEBIAN_DIR/postrm"

rm -f "$OUTPUT" "$OUTPUT.sha256"
dpkg-deb --build --root-owner-group "$ROOT" "$OUTPUT"
sha256sum "$OUTPUT" > "$OUTPUT.sha256"

echo "Built $OUTPUT"
echo "Built $OUTPUT.sha256"
