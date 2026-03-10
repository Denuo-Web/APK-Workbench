# Release builds (Linux aarch64)

AADK services and the GTK UI are only supported on Linux aarch64. Debian 13 is
the primary validated distro for full-stack smoke tests and the optional `.deb`
package flow.

Use GitHub Releases as the canonical binary distribution channel:
- Primary artifact: `aadk-${VERSION}-linux-aarch64.tar.gz`
- Required companion checksum: `aadk-${VERSION}-linux-aarch64.tar.gz.sha256`
- Optional extra artifact: `aadk_${VERSION}_arm64.deb`

## Build all binaries
```bash
cargo build --release --workspace
ls -1 target/release/aadk-*
```

## Package a release archive
```bash
VERSION=0.1.0
OUT=dist/aadk-${VERSION}-linux-aarch64
mkdir -p "${OUT}"
cp target/release/aadk-{core,workflow,toolchain,project,build,targets,observe,ui,cli} "${OUT}/"
cp scripts/release/aadk-start.sh "${OUT}/aadk-start.sh"
cp README.md LICENSE "${OUT}/"
tar -C dist -czf "aadk-${VERSION}-linux-aarch64.tar.gz" "aadk-${VERSION}-linux-aarch64"
sha256sum "aadk-${VERSION}-linux-aarch64.tar.gz" > "aadk-${VERSION}-linux-aarch64.tar.gz.sha256"
```

From the extracted folder, run:
```bash
./aadk-start.sh
```

Upload these files to a GitHub Release:
- `dist/aadk-${VERSION}-linux-aarch64.tar.gz`
- `dist/aadk-${VERSION}-linux-aarch64.tar.gz.sha256`

## Scripted release build
```bash
scripts/release/build.sh
```

Override the version:
```bash
VERSION=0.1.0 scripts/release/build.sh
```

## Debian package (.deb)
This is an additional convenience artifact for Debian-like ARM64 hosts. The
validated path is Debian 13 ARM64.

Requires `dpkg-deb` (from `dpkg-dev`) on a Debian-like ARM64 host (for example,
Debian 13 or Raspberry Pi OS 64-bit).

```bash
VERSION=0.1.0 scripts/release/build-deb.sh
```

Artifacts:
- `dist/aadk_${VERSION}_arm64.deb`
- `dist/aadk_${VERSION}_arm64.deb.sha256`

If you build it, attach both files to the same GitHub Release as optional
Debian-specific downloads.

Install:
```bash
sudo apt install ./dist/aadk_${VERSION}_arm64.deb
```

Menu entry:
- Appears under `Programming` as `AADK`.
- Runs `aadk` (services + GTK UI). Logs go to `~/.local/share/aadk/logs`.

Uninstall:
```bash
sudo apt remove aadk
```
