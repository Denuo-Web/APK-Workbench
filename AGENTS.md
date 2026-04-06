# APK Workbench - Agent Overview

## Purpose
This repository is a GUI-first, multi-service gRPC platform for Android development workflows.
It is intentionally minimal but complete enough to extend. The GTK4 UI and CLI are thin clients;
the service crates contain the real workflows. The project is designed around a JobService that
streams events to clients while long-running jobs execute in other services.

## Supported host
- Linux ARM64 (aarch64) is the only supported host for running the full stack (services/UI/Cuttlefish).
- Debian 13 on Linux ARM64 is the primary validated distro for full-stack support, release smoke
  tests, and default Cuttlefish host-tool automation.
- Non-Debian Linux ARM64 hosts are experimental for the full stack and generally require explicit
  overrides such as APKW_CUTTLEFISH_INSTALL_CMD.
- x86_64 is intentionally out of scope because Android Studio already covers it.
- Toolchain catalog includes Linux ARM64 SDK/NDK artifacts plus Windows ARM64 NDK artifacts (r29/r28c/r27d);
  no darwin SDK/NDK artifacts are published in the custom catalogs.
- Cuttlefish install uses APKW_CUTTLEFISH_INSTALL_CMD when set; Debian-like hosts fall back to the
  android-cuttlefish apt repo install command.
- GitHub Releases is the canonical binary distribution channel (`linux-aarch64.tar.gz` plus checksums);
  the Debian `.deb` is an additional convenience artifact, and GitHub Packages is not used for native binaries.
- Release packaging scripts default `VERSION` from workspace metadata, share a single binary list via
  `scripts/release/common.sh`, share Java runtime policy via `scripts/release/apkw-env.sh`, and enforce Linux ARM64 host checks unless
  `APKW_ALLOW_UNSUPPORTED_RELEASE_HOST=1` is set for explicit experimental packaging.
- Debian packages install the launcher and binaries under `/usr/lib/apkw`, expose `/usr/bin/{apkw,apkw-ui,apkw-cli}` symlinks,
  ship minimal manpages for those commands, depend on `libgtk-4-1` plus `libwebkitgtk-6.0-4` for the embedded Cuttlefish pane,
  validate `PKGNAME` before packaging, and strip staged binaries during packaging.

## Maintenance
Keep this file and the per-service AGENTS.md files in sync with code changes. When Codex changes
files, commits, or pushes, update the relevant AGENTS.md entries and adjust TODO lists to remove
completed items or move them into the implementation notes.

## Repository map
- crates/apkw-core: JobService (event streaming and job registry)
- crates/apkw-workflow: WorkflowService (multi-step pipeline orchestration)
- crates/apkw-toolchain: ToolchainService (SDK/NDK provider, install, verify)
- crates/apkw-project: ProjectService (templates, create/open, recent list)
- crates/apkw-build: BuildService (Gradle builds and artifact listing)
- crates/apkw-targets: TargetService (ADB + Cuttlefish target management)
- crates/apkw-observe: ObserveService (run history and bundle export)
- crates/apkw-ui: GTK4 GUI client
- crates/apkw-cli: CLI sanity tool
- crates/apkw-util: Shared helpers (paths, time, service bootstrap, job history)
- crates/apkw-telemetry: Opt-in telemetry spooler (usage events + crash reports)
- crates/apkw-proto: Rust gRPC codegen for proto/apkw/v1
- proto/apkw/v1/*.proto: gRPC contracts
- CHANGELOG.md: release notes and post-tag history used for version increment prep
- scripts/dev/run-all.sh: local dev runner for all services (uses the shared launcher env helper to auto-export ANDROID_SDK_ROOT/ANDROID_HOME and APKW_ADB_PATH when an SDK is detected)
- scripts/release/common.sh: shared release metadata/helpers (workspace version, supported-host guards, binary list)
- scripts/release/apkw-env.sh: shared Android/Java environment detection for the dev runner and installed launcher
- scripts/release/build.sh: release build + GitHub Releases tarball packaging helper
- scripts/release/build-deb.sh: Debian (.deb) convenience package builder (templates package metadata/docs and installs the launcher helper)
- scripts/release/apkw-start.sh: installed launcher (services + UI, logs to ~/.local/share/apkw/logs)
- packaging/deb/*: Debian packaging metadata (control, desktop entry, postinst/postrm, manpages)
- assets/apkw.svg: GTK app icon used by the Debian package
- docs/release.md: release build steps
- SampleConsole: Minimal Compose sample app (Sample Console) bundled with APKW
- CS492_Assignment1_RosenauJ/CS492A1RosenauJ: Course assignment sample app

## Runtime topology
Default addresses (override with env vars):
- Job/Core:     127.0.0.1:50051 (APKW_JOB_ADDR)
- Toolchain:    127.0.0.1:50052 (APKW_TOOLCHAIN_ADDR)
- Project:      127.0.0.1:50053 (APKW_PROJECT_ADDR)
- Build:        127.0.0.1:50054 (APKW_BUILD_ADDR)
- Targets:      127.0.0.1:50055 (APKW_TARGETS_ADDR)
- Observe:      127.0.0.1:50056 (APKW_OBSERVE_ADDR)
- Workflow:     127.0.0.1:50057 (APKW_WORKFLOW_ADDR)

## Cross-service flows
- UI/CLI call gRPC services; they do not implement business logic.
- JobService is the event bus. Toolchain/Build/Targets publish job events; UI streams events.
- Progress payloads include job-specific metrics for build/project/toolchain/target/observe workflows.
- BuildService resolves project paths via ProjectService GetProject for ids; direct paths are accepted.
- BuildService loads a Gradle model snapshot after project evaluation to validate modules/variants.
- TargetService shells out to adb and optionally Cuttlefish tooling (KVM/GPU preflight, headless WebRTC fallback, images-dir fallback logging, env-control URLs).
- TargetService GetCuttlefishStatus overlays running start/stop jobs and uses `cvd` + adb + process
  probes (`run_cvd`/`launch_cvd`) so stale host processes are reported as running instead of stopped.
- TargetService keeps `stopped` when only stale `adb offline` entries remain after stop; only
  `adb_state=device` upgrades Cuttlefish state to `running`.
- TargetService also avoids forcing `starting`/`stopping` from stale running start/stop jobs when
  runtime probes already report a stopped/not-installed state.
- TargetService Cuttlefish start auto-falls back to `--enable_tap_devices=false` on hosts where TAP
  device creation is blocked, unless explicit `--enable_tap_devices` args are provided.
- TargetService Cuttlefish start now auto-applies conservative CPU/RAM limits on smaller hosts
  unless explicit CPU/memory launch args are provided.
- TargetService Cuttlefish start now auto-applies smaller display sizing on constrained hosts
  unless explicit display args are provided.
- TargetService Cuttlefish start patches empty WebRTC `custom.css` assets in local Cuttlefish
  image dirs to avoid intermittent browser stylesheet dropouts.
- TargetService now treats the Cuttlefish Debian host packages as part of host-tool readiness:
  unless `APKW_CUTTLEFISH_START_CMD` overrides startup, `/usr/lib/cuttlefish-common/bin/capability_query.py`
  (or `APKW_CUTTLEFISH_CAPABILITY_QUERY`) must exist or install/start/status surfaces a host-tools-incomplete error.
- TargetService default Debian host installs prefer passwordless `sudo -n`, but can fall back to
  `pkexec` in graphical sessions so package reinstalls still work when the local host bundle exists
  but the system Cuttlefish helpers were removed.
- ToolchainService downloads SDK/NDK archives, verifies sha256, persists state under ~/.local/share/apkw.
- ToolchainService normalizes SDK cmdline-tools layout by creating cmdline-tools/latest links when
  archives ship a flat cmdline-tools/bin + lib layout.
- ToolchainService catalog pins SDK 36.0.0/35.0.2 and NDK r29/r28c/r27d/r26d; Linux ARM64 artifacts use
  linux-aarch64/aarch64-linux-android/aarch64_be-linux-musl, and Windows ARM64 NDK artifacts are included
  for r29/r28c/r27d (.7z).
- ToolchainService ListAvailable now merges the pinned catalog with cached upstream GitHub release
  discovery for the custom SDK/NDK providers, and CheckUpstreamReleases reports whether the local
  catalog lags the latest upstream host-compatible release.
- ToolchainService install/update/verify can use upstream-only releases when GitHub release metadata
  exposes a matching asset URL plus sha256 digest, so new upstream SDK/NDK versions do not require
  an immediate catalog edit before they can be discovered or installed.
- ObserveService persists run history and a run output inventory; run records include a summary pointer for output counts and last bundle.
- RunId is a first-class identifier for multi-service workflows; correlation_id remains a secondary grouping key.
- JobService StreamRunEvents aggregates run events across jobs using bounded buffering and best-effort timestamp ordering for late discovery.
- WorkflowService orchestrates workflow.pipeline runs and upserts run records to ObserveService.
- UI/CLI can export/import local state archives and invoke ReloadState RPCs to rehydrate service state after import.
- UI header New project runs reset-all-state then opens the project folder picker; Open project uses the picker and auto-opens existing projects.

## Shared data and locations
- Job state: ~/.local/share/apkw/state/jobs.json
- UI config: ~/.local/share/apkw/state/ui-config.json
- UI state: ~/.local/share/apkw/state/ui-state.json
- CLI config: ~/.local/share/apkw/state/cli-config.json
- Project state: ~/.local/share/apkw/state/projects.json
- Build state: ~/.local/share/apkw/state/builds.json
- Toolchain state: ~/.local/share/apkw/state/toolchains.json
- Toolchain downloads: ~/.local/share/apkw/downloads
- Toolchain installs: ~/.local/share/apkw/toolchains
- Target state: ~/.local/share/apkw/state/targets.json
- Observe state: ~/.local/share/apkw/state/observe.json
- Observe bundle outputs: ~/.local/share/apkw/bundles
- UI/CLI log exports: ~/.local/share/apkw/state/*-job-export-*.json
- State export archives: ~/.local/share/apkw/state-exports/*.zip
- State operation queue/locks: ~/.local/share/apkw/state-ops
- Telemetry events/crashes: ~/.local/share/apkw/telemetry/<app>/*

## Per-service AGENT files
- crates/apkw-project/AGENTS.md
- crates/apkw-core/AGENTS.md
- crates/apkw-workflow/AGENTS.md
- crates/apkw-observe/AGENTS.md
- crates/apkw-build/AGENTS.md
- crates/apkw-toolchain/AGENTS.md
- crates/apkw-targets/AGENTS.md
- crates/apkw-ui/AGENTS.md
- crates/apkw-cli/AGENTS.md
- crates/apkw-telemetry/AGENTS.md
- proto/apkw/v1/AGENTS.md

## Prioritized TODO checklist by service

### ProjectService (apkw-project)
- None (workflow UI relies on existing create/open APIs).

### JobService (apkw-core)
- None (StreamRunEvents already supports workflow/run dashboards).

### ObserveService (apkw-observe)
- None (run output inventory is stored and exposed via ListRunOutputs with summary pointers).

### BuildService (apkw-build)
- None.

### ToolchainService (apkw-toolchain)
- None.

### TargetService (apkw-targets)
- None.

### Clients (apkw-ui, apkw-cli)
- Add picker integrations for workflow inputs (templates, toolchain sets, targets) to reduce manual ids.
- Add run filters (project/target/toolchain/result) and output shortcuts (open/export) to Evidence dashboards.
