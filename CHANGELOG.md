# Changelog

All notable changes to APK Workbench are documented in this file.

The format is based on Keep a Changelog and the project versioning follows Semantic Versioning.

## [Unreleased]

## [0.2.1] - 2026-04-19

### Changed
- Repository metadata, release docs, and README links now point at `github.com/Denuo-Web/APK-Workbench` after the GitHub owner/repository move.
- Added an external-project Gradle wrapper at `scripts/dev/apkw-gradle.sh` and documented it as the preferred way to build sibling Android repos with APK Workbench-managed ARM64 SDK/NDK plus the correct `aapt2` override.
- APK Workbench Android environment detection now uses the current `~/.local/share/apkw/toolchains` layout consistently across the launcher and external-project Gradle wrapper.
- The external-project Gradle wrapper now prefers APK Workbench-managed toolchains over inherited shell `ANDROID_*` variables by default so stale system SDK exports do not silently push builds back onto x86-era tooling.
- Host-profile detection is now explicit across shell wrappers and TargetService: APKW exports/uses host OS metadata plus a detected 4K vs 16K page-size profile, honors `APKW_HOST_PAGE_SIZE` as an override, and passes the host profile into Gradle builds for external Android repos.
- Host-support docs now treat Debian 13 ARM64, including Raspberry Pi OS 64-bit, as the primary validated path.
- Local data/config, telemetry, and Cuttlefish bundle handling now consistently use `apkw` paths and metadata names without the previous transitional aliases.

## [0.2.0] - 2026-04-06

### Added
- Embedded Cuttlefish WebRTC viewing inside the GTK Targets page using WebKitGTK, including in-app reload support, visible current URL state, and fallback browser handoff.
- Upstream GitHub release discovery for the custom SDK and NDK providers, including merged availability results, lag checks against the pinned catalog, and support for installing or verifying upstream-only releases when URL and sha256 metadata are available.
- Debian manpages for `apkw`, `apkw-ui`, and `apkw-cli`.
- Transitional compatibility bridges for older env vars and project metadata names while the APKW naming migration landed.

### Changed
- Renamed the product to `APK Workbench` and moved the workspace package, crate, command, proto, packaging, and documentation surface to `apkw*`.
- Commands and package entry points now ship as `apkw`, `apkw-ui`, and `apkw-cli`.
- State, project metadata, and install layouts now prefer `~/.local/share/apkw`, `.apkw/project.json`, `/usr/lib/apkw`, and `apkw-*` release artifacts.
- Release packaging now shares version and binary metadata from workspace-level helpers, with common logic centralized in `scripts/release/common.sh`.
- The dev runner and installed launcher now share Android/Java environment detection through `scripts/release/apkw-env.sh`, keeping `ANDROID_SDK_ROOT`, `ANDROID_HOME`, and `APKW_ADB_PATH` behavior aligned.
- Debian package staging now installs the payload under `/usr/lib/apkw`, exposes `/usr/bin/apkw`, `/usr/bin/apkw-ui`, and `/usr/bin/apkw-cli` symlinks, validates `PKGNAME`, and strips staged binaries during packaging.
- Packaging and README guidance now treat GitHub Releases tarballs plus checksums as the canonical desktop distribution path, with the Debian package kept as an additional convenience artifact.
- Product copy, package metadata, and UI text now consistently use `APK Workbench` / `APKW` naming instead of scaffold terminology.
- The Targets page logcat action now streams from the current target field or active target instead of a hard-coded sample device id.
- `apkw-util` and `apkw-observe` now avoid unnecessary `zip` features to trim packaged binary footprint.
- `apkw-ui` now snapshots `ui-state.json` incrementally while the app is open and flushes a fresh UI snapshot before the header Save state archive action runs.
- `apkw-core` now indexes jobs by run id and correlation id so run-event discovery avoids rescanning the entire job store every poll tick.

### Fixed
- Release checksum files now record artifact basenames so `sha256sum -c` works after downloading GitHub Release assets.
- Debian packaging metadata and release docs now account for the WebKitGTK runtime needed by the embedded Cuttlefish pane.
- Build and release documentation now call out the required WebKitGTK development packages for building `apkw-ui`.
- State archive restore now stages extracted files under `state-ops` on the target filesystem, rejects archives with no restorable entries, and uses synced unique temp files for JSON state writes to reduce stale-save and restore corruption risks.
- UI log persistence now trims saved log snapshots without full-string character rescans, reducing log-heavy save overhead.

### Documentation
- Repository and service agent notes were synced with the current packaging, toolchain, and UI behavior.

## [0.1.0] - 2026-03-10

### Added
- Initial tagged `APK Workbench` release.
- Linux ARM64 release tarball packaging and Debian-first release documentation.
