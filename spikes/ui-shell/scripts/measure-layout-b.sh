#!/usr/bin/env bash
# Run from any shell, including fish. Does not build or change the user's settings.
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
if [[ "${1:-}" == "--help" ]]; then
  echo 'Usage: scripts/measure-layout-b.sh [cold|rss|frames|all] [output-directory]'
  echo 'Optional env: ATRIUM_MEASURE_BIN (absolute path), ATRIUM_MEASURE_RUNS (cold=10, frames=3 default).'
  exit 0
fi
mode="${1:-all}"
case "$mode" in cold|rss|frames|all) ;; *) echo "Unknown mode: $mode" >&2; exit 2;; esac
out="${2:-/tmp/atrium-layout-b-measurements}"
mkdir -p -- "$out"
args=()
if [[ -n "${ATRIUM_MEASURE_BIN:-}" ]]; then args+=(--bin "$ATRIUM_MEASURE_BIN"); fi
modes=("$mode")
if [[ "$mode" == all ]]; then modes=(cold rss frames); fi
for windows in 1 3; do
  for opacity in off background whole; do
    for metric in "${modes[@]}"; do
      extra=()
      if [[ "$metric" != rss && -n "${ATRIUM_MEASURE_RUNS:-}" ]]; then extra+=(--runs "$ATRIUM_MEASURE_RUNS"); fi
      echo "Measuring metric=$metric windows=$windows opacity=$opacity (alpha off=1, otherwise=0.6)"
      python3 scripts/measure.py "$metric" --windows "$windows" --opacity "$opacity" ${args[@]+"${args[@]}"} ${extra[@]+"${extra[@]}"} \
        | tee "$out/$metric-$windows-$opacity.log"
    done
  done
done
