# ToolchainService Agent Notes (apkw-toolchain)

## Role and scope
ToolchainService exposes SDK/NDK provider metadata, installs toolchains, verifies installs,
tracks installed toolchains, and manages toolchain sets. It publishes job progress to JobService
for long-running actions.

## Maintenance
Update this file whenever ToolchainService behavior changes or when commits touching this crate are made.

## gRPC contract
- proto/apkw/v1/toolchain.proto
- RPCs: ListProviders, ListAvailable, CheckUpstreamReleases, ListInstalled, ListToolchainSets, InstallToolchain,
  VerifyToolchain, UpdateToolchain, UninstallToolchain, CleanupToolchainCache,
  CreateToolchainSet, SetActiveToolchainSet, GetActiveToolchainSet, ReloadState

## Current implementation details
- Implementation is split across crates/apkw-toolchain/src/service.rs (gRPC + orchestration),
  crates/apkw-toolchain/src/catalog.rs (catalog/fixtures), crates/apkw-toolchain/src/artifacts.rs
  (download/extract), crates/apkw-toolchain/src/verify.rs (signature/transparency checks),
  crates/apkw-toolchain/src/state.rs (persisted state/toolchain sets), crates/apkw-toolchain/src/jobs.rs
  (JobService helpers), crates/apkw-toolchain/src/hashing.rs (sha256 helpers),
  crates/apkw-toolchain/src/provenance.rs (provenance I/O), and crates/apkw-toolchain/src/cancel.rs;
  crates/apkw-toolchain/src/main.rs only wires the tonic server.
- Service bootstrap, timestamps, and base data paths rely on `apkw-util` to keep defaults consistent.
- Providers and versions come from a JSON catalog (crates/apkw-toolchain/catalog.json or
  APKW_TOOLCHAIN_CATALOG override).
- The custom SDK/NDK providers also support cached upstream discovery from GitHub Releases; list
  and explicit upstream-check requests merge or compare host-compatible release metadata against the
  pinned catalog.
- Catalog pins SDK 36.0.0/35.0.2 and NDK r29/r28c/r27d/r26d for the android-* custom providers. Linux
  ARM64 artifacts use linux-aarch64 (musl), aarch64-linux-android, and aarch64_be-linux-musl; Windows
  ARM64 NDK artifacts (windows-aarch64 .7z) are included for r29/r28c/r27d. No darwin SDK/NDK
  artifacts are available in the custom catalogs.
- Host selection uses APKW_TOOLCHAIN_HOST when set and falls back to host aliases (for example,
  linux-aarch64 -> aarch64-linux-musl/aarch64-linux-android/aarch64_be-linux-musl,
  windows-aarch64 -> aarch64-w64-mingw32, and windows-x86_64 -> x86_64-w64-mingw32, plus
  APKW_TOOLCHAIN_HOST_FALLBACK when the catalog lacks a matching artifact.
- Available versions can also be sourced from fixture archives via APKW_TOOLCHAIN_FIXTURES_DIR.
- Toolchains are installed under ~/.local/share/apkw/toolchains and cached in
  ~/.local/share/apkw/downloads; state is persisted in ~/.local/share/apkw/state/toolchains.json.
- SDK installs normalize cmdline-tools layout by adding cmdline-tools/latest links when archives
  ship a flat cmdline-tools/bin + lib layout.
- Install/Update/Verify job progress metrics include provider/version/host/verify settings plus artifact URLs/paths and install roots.
- verify_toolchain checks install path, provenance contents, known-release consistency (catalog or
  upstream GitHub metadata), artifact size, optional Ed25519 signatures (over SHA256 digest),
  transparency log entries when configured, and layout; it re-fetches the artifact for hash
  verification when needed.
- Catalog artifacts can supply signature metadata via `signature`, `signature_url`, and
  `signature_public_key` (hex or base64); signatures are recorded in provenance when available.
- InstallToolchain and VerifyToolchain accept optional job_id to reuse existing JobService jobs,
  plus correlation_id and run_id to group multi-step workflows.
- InstallToolchain and VerifyToolchain trim provider/toolchain identifiers and reject empty values with invalid-argument errors.
- Update/Uninstall/Cleanup cache operations publish JobService events and can reuse job_id while
  honoring correlation_id and run_id for grouped job streams.
- Toolchain sets are persisted in ~/.local/share/apkw/state/toolchains.json along with the active
  toolchain set id.
- ReloadState reloads persisted toolchain installs and set metadata from disk.
- State load now scans ~/.local/share/apkw/toolchains for provenance files and rehydrates installed
  toolchains when state is missing, so existing installs survive reset-all-state.

## Data flow and dependencies
- Uses JobService for install/verify jobs and publishes logs/progress events.
- UI/CLI call ListProviders/ListAvailable/CheckUpstreamReleases/ListInstalled/ListToolchainSets/
  Install/Verify plus toolchain set management RPCs.

## Environment / config
- APKW_TOOLCHAIN_ADDR sets the bind address (default 127.0.0.1:50052).
- APKW_JOB_ADDR sets the JobService address.
- APKW_TOOLCHAIN_FIXTURES_DIR points to local fixture archives for offline dev.
- APKW_TOOLCHAIN_CATALOG overrides the provider catalog path.
- APKW_TOOLCHAIN_HOST overrides detected host (e.g., linux-aarch64, linux-x86_64).
- APKW_TOOLCHAIN_HOST_FALLBACK provides a comma-separated fallback host list.
- Upstream GitHub release metadata is cached in-memory for five minutes per provider.
- APKW_TELEMETRY and APKW_TELEMETRY_CRASH enable opt-in usage/crash reporting (local spool).

## Telemetry
- Emits service.start (service=toolchain) when opt-in telemetry is enabled.

## Implementation notes
- InstallToolchain clones artifact URL/hash when persisting InstalledToolchain so post-install metrics can reuse artifact metadata.
- Archive extraction supports .7z via `7z` when needed (Windows ARM64 NDK artifacts).
- Toolchain sources are kept rustfmt-formatted to align with workspace style.

## Prioritized TODO checklist by service
- None (workflow UI consumes existing toolchain RPCs).
