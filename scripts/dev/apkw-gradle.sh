#!/usr/bin/env bash
set -euo pipefail

SCRIPT_PATH="$(readlink -f "${BASH_SOURCE[0]}")"
SCRIPT_DIR="$(cd "$(dirname "$SCRIPT_PATH")" && pwd)"
# shellcheck source=scripts/release/apkw-env.sh
source "${SCRIPT_DIR}/../release/apkw-env.sh"

usage() {
  cat <<'EOF'
Usage:
  apkw-gradle.sh [--project-dir DIR] <gradle-args...>
  apkw-gradle.sh --print-env

Examples:
  apkw-gradle.sh --project-dir /path/to/app assembleDebug
  apkw-gradle.sh --project-dir /path/to/app :app:compileDebugKotlin
EOF
}

PROJECT_DIR="$PWD"
PRINT_ENV=0

while [ $# -gt 0 ]; do
  case "$1" in
    --project-dir)
      [ $# -ge 2 ] || {
        echo "ERROR: --project-dir requires a path." >&2
        exit 2
      }
      PROJECT_DIR="$2"
      shift 2
      ;;
    --print-env)
      PRINT_ENV=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    --)
      shift
      break
      ;;
    *)
      break
      ;;
  esac
done

if [ "${APKW_GRADLE_RESPECT_EXISTING_ENV:-0}" != "1" ]; then
  unset ANDROID_SDK_ROOT ANDROID_HOME ANDROID_NDK_ROOT ANDROID_NDK_HOME APKW_ADB_PATH
fi

apkw_prepare_launch_env
apkw_sync_sdk_platforms_from_system

AAPT2_BIN=""
if [ -n "${APKW_AAPT2_PATH:-}" ] && [ -x "${APKW_AAPT2_PATH}" ]; then
  AAPT2_BIN="${APKW_AAPT2_PATH}"
elif [ -n "${ANDROID_SDK_ROOT:-}" ]; then
  AAPT2_BIN="$(apkw_pick_latest_aapt2 "$ANDROID_SDK_ROOT" || true)"
fi

if [ "$PRINT_ENV" -eq 1 ]; then
  echo "ANDROID_SDK_ROOT=${ANDROID_SDK_ROOT:-}"
  echo "ANDROID_HOME=${ANDROID_HOME:-}"
  echo "ANDROID_NDK_ROOT=${ANDROID_NDK_ROOT:-}"
  echo "ANDROID_NDK_HOME=${ANDROID_NDK_HOME:-}"
  echo "JAVA_HOME=${JAVA_HOME:-}"
  echo "APKW_ADB_PATH=${APKW_ADB_PATH:-}"
  echo "APKW_AAPT2_PATH=${AAPT2_BIN:-}"
  exit 0
fi

[ $# -gt 0 ] || {
  usage >&2
  exit 2
}

[ -d "$PROJECT_DIR" ] || {
  echo "ERROR: project dir does not exist: $PROJECT_DIR" >&2
  exit 2
}

GRADLE_BIN=""
if [ -x "$PROJECT_DIR/gradlew" ]; then
  GRADLE_BIN="$PROJECT_DIR/gradlew"
elif command -v gradle >/dev/null 2>&1; then
  GRADLE_BIN="$(command -v gradle)"
else
  echo "ERROR: no Gradle wrapper or system gradle found for $PROJECT_DIR" >&2
  exit 2
fi

EXTRA_ARGS=("--console=plain")
if [ -n "$AAPT2_BIN" ]; then
  EXTRA_ARGS+=("-Pandroid.aapt2FromMavenOverride=$AAPT2_BIN")
fi

echo "APK Workbench Gradle wrapper"
echo "  project: $PROJECT_DIR"
echo "  gradle:  $GRADLE_BIN"
echo "  sdk:     ${ANDROID_SDK_ROOT:-<unset>}"
echo "  ndk:     ${ANDROID_NDK_ROOT:-<unset>}"
echo "  java:    ${JAVA_HOME:-<unset>}"
echo "  aapt2:   ${AAPT2_BIN:-<unset>}"
echo

cd "$PROJECT_DIR"
exec "$GRADLE_BIN" "${EXTRA_ARGS[@]}" "$@"
