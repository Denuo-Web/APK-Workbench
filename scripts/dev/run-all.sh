#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/release/apkw-env.sh
source "${SCRIPT_DIR}/../release/apkw-env.sh"

export APKW_JOB_ADDR="${APKW_JOB_ADDR:-127.0.0.1:50051}"
export APKW_TOOLCHAIN_ADDR="${APKW_TOOLCHAIN_ADDR:-127.0.0.1:50052}"
export APKW_PROJECT_ADDR="${APKW_PROJECT_ADDR:-127.0.0.1:50053}"
export APKW_BUILD_ADDR="${APKW_BUILD_ADDR:-127.0.0.1:50054}"
export APKW_TARGETS_ADDR="${APKW_TARGETS_ADDR:-127.0.0.1:50055}"
export APKW_OBSERVE_ADDR="${APKW_OBSERVE_ADDR:-127.0.0.1:50056}"
export APKW_WORKFLOW_ADDR="${APKW_WORKFLOW_ADDR:-127.0.0.1:50057}"

apkw_prepare_launch_env
apkw_print_launch_env_summary

pids=()

cleanup() {
  echo "Stopping services..."
  for pid in "${pids[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  wait || true
}
trap cleanup EXIT INT TERM

echo "Starting apkw-core (JobService) on $APKW_JOB_ADDR"
cargo run -p apkw-core --quiet &
pids+=($!)

echo "Starting apkw-toolchain on $APKW_TOOLCHAIN_ADDR"
cargo run -p apkw-toolchain --quiet &
pids+=($!)

echo "Starting apkw-project on $APKW_PROJECT_ADDR"
cargo run -p apkw-project --quiet &
pids+=($!)

echo "Starting apkw-build on $APKW_BUILD_ADDR"
cargo run -p apkw-build --quiet &
pids+=($!)

echo "Starting apkw-targets on $APKW_TARGETS_ADDR"
cargo run -p apkw-targets --quiet &
pids+=($!)

echo "Starting apkw-observe on $APKW_OBSERVE_ADDR"
cargo run -p apkw-observe --quiet &
pids+=($!)

echo "Starting apkw-workflow on $APKW_WORKFLOW_ADDR"
cargo run -p apkw-workflow --quiet &
pids+=($!)

echo
echo "All services started."
echo "Press Ctrl+C to stop."
echo

wait
