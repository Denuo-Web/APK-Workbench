# Changelog

All notable changes to AADK Full are documented in this file.

The format is based on Keep a Changelog and the project versioning follows Semantic Versioning.

## [Unreleased]

Changes in this section cover everything merged after `v0.1.0` on 2026-03-10.

### Added
- Embedded Cuttlefish WebRTC viewing inside the GTK Targets page using WebKitGTK, including in-app reload support, visible current URL state, and fallback browser handoff.
- Upstream GitHub release discovery for the custom SDK and NDK providers, including merged availability results, lag checks against the pinned catalog, and support for installing or verifying upstream-only releases when URL and sha256 metadata are available.
- Debian manpages for `aadk`, `aadk-ui`, and `aadk-cli`.

### Changed
- Release packaging now shares version and binary metadata from workspace-level helpers, with common logic centralized in `scripts/release/common.sh`.
- The dev runner and installed launcher now share Android/Java environment detection through `scripts/release/aadk-env.sh`, keeping `ANDROID_SDK_ROOT`, `ANDROID_HOME`, and `AADK_ADB_PATH` behavior aligned.
- Debian package staging now installs the payload under `/usr/lib/aadk`, exposes `/usr/bin/aadk`, `/usr/bin/aadk-ui`, and `/usr/bin/aadk-cli` symlinks, validates `PKGNAME`, and strips staged binaries during packaging.
- Packaging and README guidance now treat GitHub Releases tarballs plus checksums as the canonical desktop distribution path, with the Debian package kept as an additional convenience artifact.
- Product copy, package metadata, and UI text now consistently use `AADK` naming instead of scaffold terminology.
- `aadk-util` and `aadk-observe` now avoid unnecessary `zip` features to trim packaged binary footprint.

### Fixed
- Debian packaging metadata and release docs now account for the WebKitGTK runtime needed by the embedded Cuttlefish pane.
- Build and release documentation now call out the required WebKitGTK development packages for building `aadk-ui`.

### Documentation
- Repository and service agent notes were synced with the current packaging, toolchain, and UI behavior.

### Commit Summary Since v0.1.0
- `2026-04-03 bc37727` `release: centralize packaging metadata`
- `2026-04-03 3532432` `branding: remove scaffold terminology`
- `2026-04-03 0bdcb24` `release: centralize launcher env and deb metadata`
- `2026-04-03 e37d275` `toolchain: discover upstream SDK and NDK releases`
- `2026-04-03 9e2ec1e` `docs: sync repo agent notes`
- `2026-04-03 565f04a` `packaging: clean up Debian install layout`
- `2026-04-03 2109880` `util: trim zip features for packaged binaries`
- `2026-04-04 3c4b316` `aadk-ui: embed the Cuttlefish WebRTC view`
- `2026-04-04 0672182` `packaging: document WebKitGTK requirements`

## [0.1.0] - 2026-03-10

### Added
- Initial tagged `AADK Full` release.
- Linux ARM64 release tarball packaging and Debian-first release documentation.
