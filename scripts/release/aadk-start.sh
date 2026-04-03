#!/usr/bin/env bash
set -euo pipefail

MODE="ui"
if [ "${1:-}" = "--services" ] || [ "${1:-}" = "--no-ui" ]; then
  MODE="services"
  shift
elif [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
  echo "Usage: aadk-start [--services] [ui-args...]"
  echo "  --services  Start services only (no UI)."
  exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/release/aadk-env.sh
source "${SCRIPT_DIR}/aadk-env.sh"

DEFAULT_BIN_DIR="/opt/aadk/bin"
BIN_DIR="${AADK_BIN_DIR:-$DEFAULT_BIN_DIR}"
if [ -z "${AADK_BIN_DIR:-}" ] && [ -x "$SCRIPT_DIR/aadk-core" ]; then
  BIN_DIR="$SCRIPT_DIR"
fi
LOG_DIR="${XDG_STATE_HOME:-$HOME/.local/share}/aadk/logs"
mkdir -p "$LOG_DIR"

if [ ! -t 1 ]; then
  exec >>"$LOG_DIR/aadk-start.log" 2>&1
fi

export AADK_JOB_ADDR="${AADK_JOB_ADDR:-127.0.0.1:50051}"
export AADK_TOOLCHAIN_ADDR="${AADK_TOOLCHAIN_ADDR:-127.0.0.1:50052}"
export AADK_PROJECT_ADDR="${AADK_PROJECT_ADDR:-127.0.0.1:50053}"
export AADK_BUILD_ADDR="${AADK_BUILD_ADDR:-127.0.0.1:50054}"
export AADK_TARGETS_ADDR="${AADK_TARGETS_ADDR:-127.0.0.1:50055}"
export AADK_OBSERVE_ADDR="${AADK_OBSERVE_ADDR:-127.0.0.1:50056}"
export AADK_WORKFLOW_ADDR="${AADK_WORKFLOW_ADDR:-127.0.0.1:50057}"

aadk_prepare_launch_env
aadk_print_launch_env_summary

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

check_bin "$BIN_DIR/aadk-core"
check_bin "$BIN_DIR/aadk-toolchain"
check_bin "$BIN_DIR/aadk-project"
check_bin "$BIN_DIR/aadk-build"
check_bin "$BIN_DIR/aadk-targets"
check_bin "$BIN_DIR/aadk-observe"
check_bin "$BIN_DIR/aadk-workflow"

start_service "aadk-core" "$BIN_DIR/aadk-core"
start_service "aadk-toolchain" "$BIN_DIR/aadk-toolchain"
start_service "aadk-project" "$BIN_DIR/aadk-project"
start_service "aadk-build" "$BIN_DIR/aadk-build"
start_service "aadk-targets" "$BIN_DIR/aadk-targets"
start_service "aadk-observe" "$BIN_DIR/aadk-observe"
start_service "aadk-workflow" "$BIN_DIR/aadk-workflow"

echo
if [ "$MODE" = "ui" ]; then
  check_bin "$BIN_DIR/aadk-ui"
  echo "Starting aadk-ui. Logs: $LOG_DIR/aadk-ui.log"
  echo
  ui_status=0
  "$BIN_DIR/aadk-ui" "$@" >>"$LOG_DIR/aadk-ui.log" 2>&1 || ui_status=$?
  exit "$ui_status"
fi

echo "All services started. Press Ctrl+C to stop."
wait
