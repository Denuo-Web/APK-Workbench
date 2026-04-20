# Release builds (Linux aarch64)

APK Workbench services and the GTK UI are only supported on Linux aarch64. Debian 13 is
the primary validated distro for full-stack smoke tests and the optional `.deb`
package flow, including Raspberry Pi OS 64-bit.

Canonical repository: `https://github.com/Denuo-Web/APK-Workbench`

Use GitHub Releases as the canonical binary distribution channel:
- Primary artifact: `apkw-${VERSION}-linux-aarch64.tar.gz`
- Required companion checksum: `apkw-${VERSION}-linux-aarch64.tar.gz.sha256`
- Optional extra artifact: `apkw_${VERSION}_arm64.deb`

## Build all binaries
```bash
cargo build --release --workspace --locked
```

For `apkw-ui`, the host also needs the GTK4 and WebKitGTK development packages
available to `pkg-config`; on Debian 13 that means `libgtk-4-dev` and
`libwebkitgtk-6.0-dev`.

## Package a release archive
`scripts/release/build.sh` is the canonical tarball flow. It defaults
`VERSION` from `[workspace.package]` in `Cargo.toml`, validates Linux aarch64
hosts by default, cleans the staging directory before repackaging, and writes
artifacts under `dist/`.

```bash
scripts/release/build.sh
```

Override the version:
```bash
VERSION=0.1.0 scripts/release/build.sh
```

Use `APKW_ALLOW_UNSUPPORTED_RELEASE_HOST=1` only for explicit experimental
packaging on unsupported hosts.

From the extracted folder, run:
```bash
./apkw-start.sh
```

Upload these files to a GitHub Release in `Denuo-Web/APK-Workbench`:
- `dist/apkw-${VERSION}-linux-aarch64.tar.gz`
- `dist/apkw-${VERSION}-linux-aarch64.tar.gz.sha256`

## Scripted release build
The shared binary list lives in `scripts/release/common.sh`, so the tarball and
Debian packaging flows stay aligned.

## Debian package (.deb)
This is an additional convenience artifact for Debian-like ARM64 hosts. The
validated path is Debian 13 ARM64.

Requires `dpkg-deb` (from `dpkg-dev`) on a Debian-like ARM64 host (for example,
Debian 13 or Raspberry Pi OS 64-bit).

`scripts/release/build-deb.sh` shares the same workspace-version default and
binary list as the tarball builder, templates `PKGNAME` through the Debian
control metadata, and derives Java runtime recommendations from the shared
launcher environment policy. It validates `PKGNAME` before packaging, strips
packaged binaries during staging, and checks that the Debian architecture
matches `ARCH` (default `arm64`) unless
`APKW_ALLOW_UNSUPPORTED_RELEASE_HOST=1`.

```bash
scripts/release/build-deb.sh
```

Override the version:
```bash
VERSION=0.1.0 scripts/release/build-deb.sh
```

Override the package name:
```bash
PKGNAME=apkw-nightly scripts/release/build-deb.sh
```

Default artifacts:
- `dist/apkw_${VERSION}_arm64.deb`
- `dist/apkw_${VERSION}_arm64.deb.sha256`

When `PKGNAME` is overridden, the `.deb` filename and the package name used for
install/remove commands change to match it.

If you build it, attach both files to the same GitHub Release in
`Denuo-Web/APK-Workbench` as optional
Debian-specific downloads.

Install:
```bash
sudo apt install ./dist/apkw_${VERSION}_arm64.deb
```

The Debian package pulls the GTK4 and WebKitGTK runtime libraries needed for
the embedded Cuttlefish pane automatically.

Menu entry:
- Appears under `Development` as `APK Workbench`.
- Runs `apkw` (services + GTK UI). Logs go to `~/.local/share/apkw/logs`.

Installed layout:
- Versionless package payload lives under `/usr/lib/apkw`.
- `/usr/bin/apkw`, `/usr/bin/apkw-ui`, and `/usr/bin/apkw-cli` are symlinked entry points.
- Minimal manpages are installed for `apkw(1)`, `apkw-ui(1)`, and `apkw-cli(1)`.

Uninstall:
```bash
sudo apt remove apkw
```
