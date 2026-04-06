# TargetService Agent Notes (apkw-targets)

## Role and scope
TargetService enumerates devices/emulators via adb, exposes optional Cuttlefish integration,
installs APKs, launches/stops apps, and streams logcat. It publishes progress/logs to JobService
for long-running actions (install/start/stop/launch/logcat/cuttlefish actions).

## Maintenance
Update this file whenever TargetService behavior changes or when commits touching this crate are made.

## gRPC contract
- proto/apkw/v1/target.proto
- RPCs: ListTargets, SetDefaultTarget, GetDefaultTarget, InstallApk, Launch, StopApp,
  StreamLogcat, InstallCuttlefish, ResolveCuttlefishBuild, StartCuttlefish, StopCuttlefish, GetCuttlefishStatus, ReloadState

## Current implementation details
- Implementation is split across crates/apkw-targets/src/service.rs (gRPC + job orchestration),
  crates/apkw-targets/src/adb.rs (adb discovery/props), crates/apkw-targets/src/cuttlefish.rs
  (Cuttlefish config/build resolution/jobs), crates/apkw-targets/src/state.rs (persisted state),
  crates/apkw-targets/src/ids.rs (normalization), and crates/apkw-targets/src/jobs.rs (JobService helpers);
  crates/apkw-targets/src/main.rs only wires the tonic server.
- Service bootstrap, timestamps, and base data paths rely on `apkw-util` for consistent defaults.
- list_targets uses a provider pipeline (ADB listing plus Cuttlefish augmentation), normalizes
  target IDs/addresses, enriches metadata/health, and merges persisted inventory for offline targets.
- default target and target inventory are persisted under ~/.local/share/apkw/state/targets.json
  and reconciled against live discovery when possible.
- APK install/launch/stop and logcat are implemented via adb commands.
- Cuttlefish install uses APKW_CUTTLEFISH_INSTALL_CMD when set; otherwise Debian-like hosts use the
  android-cuttlefish apt repo install command. Debian 13 ARM64 is the validated path for that
  automation; other distros require a custom install command and are currently experimental.
  Per-request branch/target/build_id overrides are supported via ResolveCuttlefishBuild.
- Cuttlefish operations run external commands and report state via JobService events.
- GetCuttlefishStatus now combines `cvd status` with ADB state fallback (including `adb devices -l`
  fallback when direct `adb -s <serial> get-state` fails) so running/booting instances are
  reported even when `cvd` is unavailable.
- GetCuttlefishStatus also probes `/proc` for active `run_cvd`/`launch_cvd` processes tied to the
  configured system image directory; this prevents false `stopped` reports when `cvd status`
  returns "no device" but Cuttlefish host processes are still running.
- GetCuttlefishStatus also reflects in-flight Cuttlefish jobs from JobService (`starting`/`stopping`)
  when start/stop jobs are running, so clients do not show stale `stopped` while operations are active.
- GetCuttlefishStatus no longer upgrades `stopped` to `starting` from stale `adb offline` residue
  after stop; only confirmed `adb_state=device` forces `running`.
- Running Cuttlefish start/stop jobs older than 10 minutes are ignored when computing
  start/stop preconditions and status overlays so stale persisted job state (for example after a
  crash/restart) does not pin status as `starting`/`stopping`.
- Even when a start/stop job still appears as running in JobService, status now keeps `stopped`
  for stopped/not-installed runtime snapshots and marks those job refs as stale details instead of
  forcing `starting`/`stopping`.
- StartCuttlefish now rejects duplicate starts when Cuttlefish is already detected as running or
  starting, returning a failed-precondition gRPC error instead of launching another instance.
- StartCuttlefish also rejects duplicate requests when an existing `targets.cuttlefish.start` job
  is already in `running` state, preventing concurrent start jobs even when runtime status probes
  still report stopped.
- StartCuttlefish rejects requests while a `targets.cuttlefish.stop` job is running; StopCuttlefish
  rejects duplicate stop requests while a `targets.cuttlefish.stop` job is already running.
- Start job recovery now handles stale instance-directory lock errors (for example "Instance
  directory files in use. Try `cvd reset`") by attempting stop/reset cleanup and retrying launch.
- Start command execution now uses a configurable timeout (default 120s via
  `APKW_CUTTLEFISH_START_CMD_TIMEOUT_SECS`); if it times out, the job continues with adb readiness
  checks instead of hanging indefinitely at the start phase.
- Start launch arguments now auto-disable TAP networking (`--enable_tap_devices=false`) when host
  TAP creation probes indicate insufficient permissions (for example `Operation not permitted`);
  explicit `--enable_tap_devices=...` in `APKW_CUTTLEFISH_START_ARGS` takes precedence.
- Start also auto-applies host-tier CPU/RAM launch limits when explicit values are not provided
  (for 4-core/~8GB hosts this now maps to `--cpus=2 --memory_mb=3072`); env overrides are available.
- Start also auto-applies host-tier display sizing (`x_res/y_res/dpi`) when explicit values are
  not provided (for 4-core/~8GB hosts this now maps to `720x1280@280dpi`) to avoid oversized windows.
- Start patches empty `usr/share/webrtc/assets/custom.css` files in the Cuttlefish image directory
  to avoid intermittent Web UI stylesheet dropouts caused by zero-byte CSS responses.
- Job progress metrics include target/app identifiers, adb serials, install/launch inputs, and
  target health/ABI/SDK details plus Cuttlefish env/artifact details.
- InstallApk, Launch, StopApp, and Cuttlefish job RPCs accept optional job_id for existing jobs
  plus correlation_id and run_id for grouping related workflows.
- Cuttlefish start preflight checks host tools, images, and KVM availability/access (configurable) and logs images-dir fallback/missing hints.
- Host-tool readiness now requires more than a bundled `launch_cvd` binary: unless `APKW_CUTTLEFISH_START_CMD` overrides startup, the service also expects `/usr/lib/cuttlefish-common/bin/capability_query.py` (or `APKW_CUTTLEFISH_CAPABILITY_QUERY`) so removed Debian host packages are detected as incomplete instead of being treated as installed.
- InstallCuttlefish now re-validates host readiness after the host-install phase and fails with a clear host-tools-incomplete error when the capability-query script is missing, preventing false "installed" successes.
- InstallCuttlefish now prefers passwordless `sudo -n` for the default apt-based host install, but falls back to `pkexec` in graphical sessions so desktop users can approve package installation without restarting the whole stack as root.
- Defaults align with aosp-android-latest-release and aosp_cf_*_only_phone-userdebug targets; 16K hosts use main-16k-with-phones with aosp_cf_arm64/aosp_cf_x86_64.
- GPU mode can be set via APKW_CUTTLEFISH_GPU_MODE and is appended to launch arguments when starting Cuttlefish.
- Start adds --start_webrtc based on show_full_ui or headless display detection unless already provided in APKW_CUTTLEFISH_START_ARGS.
- Cuttlefish details and job outputs include WebRTC and environment control URLs.
- ReloadState reloads persisted targets/defaults from ~/.local/share/apkw/state/targets.json.
- Target sources are kept rustfmt-formatted to align with workspace style.

## Data flow and dependencies
- Requires JobService to publish job state/log/progress for install/launch/stop/cuttlefish jobs.
- UI/CLI typically call list_targets with include_offline=true.

## Environment / config
- APKW_TARGETS_ADDR sets the bind address (default 127.0.0.1:50055).
- APKW_JOB_ADDR sets the JobService address.
- APKW_ADB_PATH or ANDROID_SDK_ROOT/ANDROID_HOME can override adb lookup.
- APKW_TELEMETRY and APKW_TELEMETRY_CRASH enable opt-in usage/crash reporting (local spool).
- scripts/dev/run-all.sh auto-exports APKW_ADB_PATH when ANDROID_SDK_ROOT is detected.

## Telemetry
- Emits service.start (service=targets) when opt-in telemetry is enabled.

### Cuttlefish configuration (env vars)
- APKW_CUTTLEFISH_ENABLE=0 to disable detection
- APKW_CVD_BIN=/path/to/cvd
- APKW_LAUNCH_CVD_BIN=/path/to/launch_cvd
- APKW_STOP_CVD_BIN=/path/to/stop_cvd
- APKW_CUTTLEFISH_ADB_SERIAL=127.0.0.1:6520
- APKW_CUTTLEFISH_CONNECT=0 to skip adb connect
- APKW_CUTTLEFISH_WEBRTC_URL=https://localhost:8443
- APKW_CUTTLEFISH_ENV_URL=https://localhost:1443
- APKW_CUTTLEFISH_PAGE_SIZE_CHECK=0 to skip kernel page-size checks
- APKW_CUTTLEFISH_KVM_CHECK=0 to skip KVM availability/access checks
- APKW_CUTTLEFISH_GPU_MODE=gfxstream|drm_virgl to configure GPU acceleration mode
- APKW_CUTTLEFISH_HOME=/path (or _16K/_4K variants)
- APKW_CUTTLEFISH_IMAGES_DIR=/path (or _16K/_4K variants)
- APKW_CUTTLEFISH_HOST_DIR=/path (or _16K/_4K variants)
- APKW_CUTTLEFISH_CAPABILITY_QUERY=/path/to/capability_query.py
- APKW_CUTTLEFISH_START_CMD / APKW_CUTTLEFISH_START_ARGS
- APKW_CUTTLEFISH_AUTO_RESOURCES=1|0
- APKW_CUTTLEFISH_CPUS=<n>
- APKW_CUTTLEFISH_MEMORY_MB=<mb>
- APKW_CUTTLEFISH_AUTO_DISPLAY=1|0
- APKW_CUTTLEFISH_X_RES=<px>
- APKW_CUTTLEFISH_Y_RES=<px>
- APKW_CUTTLEFISH_DPI=<n>
- APKW_CUTTLEFISH_TAP_MODE=auto|enabled|disabled
- APKW_CUTTLEFISH_ENABLE_TAP=1|0 (legacy alias)
- APKW_CUTTLEFISH_STOP_CMD
- APKW_CUTTLEFISH_RESET_CMD
- APKW_CUTTLEFISH_START_CMD_TIMEOUT_SECS (default 120)
- APKW_CUTTLEFISH_STOP_CMD_TIMEOUT_SECS (default 60)
- APKW_CUTTLEFISH_INSTALL_CMD (optional override; required on non-Debian hosts, where full-stack support is experimental)
- APKW_CUTTLEFISH_INSTALL_HOST=0
- APKW_CUTTLEFISH_INSTALL_IMAGES=0
- APKW_CUTTLEFISH_ADD_GROUPS=0
- APKW_CUTTLEFISH_BRANCH / APKW_CUTTLEFISH_TARGET / APKW_CUTTLEFISH_BUILD_ID

## Prioritized TODO checklist by service
- None (workflow UI consumes existing target RPCs).
