#!/usr/bin/env bash
set -euo pipefail

MODE="ui"
if [ "${1:-}" = "--services" ] || [ "${1:-}" = "--no-ui" ]; then
  MODE="services"
  shift
elif [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
  echo "Usage: apkw-start [--services] [ui-args...]"
  echo "  --services  Start services only (no UI)."
  exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/release/apkw-env.sh
source "${SCRIPT_DIR}/apkw-env.sh"

DEFAULT_BIN_DIR="/usr/lib/apkw/bin"
BIN_DIR="${APKW_BIN_DIR:-$DEFAULT_BIN_DIR}"
if [ -z "${APKW_BIN_DIR:-}" ] && [ -x "$SCRIPT_DIR/apkw-core" ]; then
  BIN_DIR="$SCRIPT_DIR"
fi
LOG_DIR="${XDG_STATE_HOME:-$HOME/.local/share}/apkw/logs"
mkdir -p "$LOG_DIR"

if [ ! -t 1 ]; then
  exec >>"$LOG_DIR/apkw-start.log" 2>&1
fi

export APKW_JOB_ADDR="${APKW_JOB_ADDR:-127.0.0.1:50051}"
export APKW_TOOLCHAIN_ADDR="${APKW_TOOLCHAIN_ADDR:-127.0.0.1:50052}"
export APKW_PROJECT_ADDR="${APKW_PROJECT_ADDR:-127.0.0.1:50053}"
export APKW_BUILD_ADDR="${APKW_BUILD_ADDR:-127.0.0.1:50054}"
export APKW_TARGETS_ADDR="${APKW_TARGETS_ADDR:-127.0.0.1:50055}"
export APKW_OBSERVE_ADDR="${APKW_OBSERVE_ADDR:-127.0.0.1:50056}"
export APKW_WORKFLOW_ADDR="${APKW_WORKFLOW_ADDR:-127.0.0.1:50057}"

apkw_prepare_launch_env
apkw_print_launch_env_summary

check_bin() {
  local bin="$1"
  if [ ! -x "$bin" ]; then
    echo "ERROR: missing executable $bin"
    exit 1
  fi
}

start_service() {
  local name="$1"
  local bin="$2"
  local log="$LOG_DIR/${name}.log"
  "$bin" >>"$log" 2>&1 &
  pids+=($!)
}

pids=()

cleanup() {
  echo "Stopping services..."
  for pid in "${pids[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  wait || true
}
trap cleanup EXIT INT TERM

check_bin "$BIN_DIR/apkw-core"
check_bin "$BIN_DIR/apkw-toolchain"
check_bin "$BIN_DIR/apkw-project"
check_bin "$BIN_DIR/apkw-build"
check_bin "$BIN_DIR/apkw-targets"
check_bin "$BIN_DIR/apkw-observe"
check_bin "$BIN_DIR/apkw-workflow"

start_service "apkw-core" "$BIN_DIR/apkw-core"
start_service "apkw-toolchain" "$BIN_DIR/apkw-toolchain"
start_service "apkw-project" "$BIN_DIR/apkw-project"
start_service "apkw-build" "$BIN_DIR/apkw-build"
start_service "apkw-targets" "$BIN_DIR/apkw-targets"
start_service "apkw-observe" "$BIN_DIR/apkw-observe"
start_service "apkw-workflow" "$BIN_DIR/apkw-workflow"

echo
if [ "$MODE" = "ui" ]; then
  check_bin "$BIN_DIR/apkw-ui"
  echo "Starting apkw-ui. Logs: $LOG_DIR/apkw-ui.log"
  echo
  ui_status=0
  "$BIN_DIR/apkw-ui" "$@" >>"$LOG_DIR/apkw-ui.log" 2>&1 || ui_status=$?
  exit "$ui_status"
fi

echo "All services started. Press Ctrl+C to stop."
wait
