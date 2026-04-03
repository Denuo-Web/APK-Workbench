#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/release/aadk-env.sh
source "${SCRIPT_DIR}/../release/aadk-env.sh"

export AADK_JOB_ADDR="${AADK_JOB_ADDR:-127.0.0.1:50051}"
export AADK_TOOLCHAIN_ADDR="${AADK_TOOLCHAIN_ADDR:-127.0.0.1:50052}"
export AADK_PROJECT_ADDR="${AADK_PROJECT_ADDR:-127.0.0.1:50053}"
export AADK_BUILD_ADDR="${AADK_BUILD_ADDR:-127.0.0.1:50054}"
export AADK_TARGETS_ADDR="${AADK_TARGETS_ADDR:-127.0.0.1:50055}"
export AADK_OBSERVE_ADDR="${AADK_OBSERVE_ADDR:-127.0.0.1:50056}"
export AADK_WORKFLOW_ADDR="${AADK_WORKFLOW_ADDR:-127.0.0.1:50057}"

aadk_prepare_launch_env
aadk_print_launch_env_summary

pids=()

cleanup() {
  echo "Stopping services..."
  for pid in "${pids[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  wait || true
}
trap cleanup EXIT INT TERM

echo "Starting aadk-core (JobService) on $AADK_JOB_ADDR"
cargo run -p aadk-core --quiet &
pids+=($!)

echo "Starting aadk-toolchain on $AADK_TOOLCHAIN_ADDR"
cargo run -p aadk-toolchain --quiet &
pids+=($!)

echo "Starting aadk-project on $AADK_PROJECT_ADDR"
cargo run -p aadk-project --quiet &
pids+=($!)

echo "Starting aadk-build on $AADK_BUILD_ADDR"
cargo run -p aadk-build --quiet &
pids+=($!)

echo "Starting aadk-targets on $AADK_TARGETS_ADDR"
cargo run -p aadk-targets --quiet &
pids+=($!)

echo "Starting aadk-observe on $AADK_OBSERVE_ADDR"
cargo run -p aadk-observe --quiet &
pids+=($!)

echo "Starting aadk-workflow on $AADK_WORKFLOW_ADDR"
cargo run -p aadk-workflow --quiet &
pids+=($!)

echo
echo "All services started."
echo "Press Ctrl+C to stop."
echo

wait
