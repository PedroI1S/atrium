#!/usr/bin/env bash
# Runs the layout-B measurement matrix: off / background / whole x 1 / 3 windows, for each metric.
# Works from any shell, including fish. Does not build and never touches the user's settings.
#
#   scripts/measure-layout-b.sh [cold|rss|frames|all] [output-directory]
#
# Optional env (fish: `env ATRIUM_MEASURE_RUNS=3 scripts/measure-layout-b.sh cold`):
#   ATRIUM_MEASURE_BIN   absolute path of the app binary (default: the release bundle)
#   ATRIUM_MEASURE_RUNS  launches per cell (cold=10, frames=3 by default)
#
# Measurement windows open on the screen. Hold the shared GUI lock so runs never overlap
# another agent's windows (overlapping windows throttle rAF):
#   /usr/bin/lockf -k /tmp/atrium-gui-measure.lock scripts/measure-layout-b.sh all
#
# A failing cell is reported with its reason and the matrix continues; the exit status is 1 if
# any cell failed. `python3 scripts/summarize.py <output-directory>` prints the result table.
set -uo pipefail
cd -- "$(dirname -- "$0")/.."
if [[ "${1:-}" == "--help" ]]; then
  sed -n '2,17p' "$0"
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
failed=()
for metric in "${modes[@]}"; do
  if [[ "$metric" == cold ]]; then
    # Interleaved: every launch round visits all six configurations, so other load on the
    # machine spreads evenly instead of skewing whichever cell happened to run during it.
    extra=()
    if [[ -n "${ATRIUM_MEASURE_RUNS:-}" ]]; then extra+=(--runs "$ATRIUM_MEASURE_RUNS"); fi
    echo "=== metric=cold, all six configurations interleaved"
    if ! python3 scripts/measure.py coldmatrix ${args[@]+"${args[@]}"} ${extra[@]+"${extra[@]}"} >"$out/cold-matrix.log" 2>&1; then
      failed+=(cold); echo "    FAILED: $(grep -m1 '^FAILED' "$out/cold-matrix.log" || tail -n 3 "$out/cold-matrix.log")"
    else
      grep '^TTI' "$out/cold-matrix.log" | sed 's/^/    ok: /'
    fi
    continue
  fi
  for windows in 1 3; do
    for opacity in off background whole; do
      extra=()
      if [[ "$metric" != rss && -n "${ATRIUM_MEASURE_RUNS:-}" ]]; then extra+=(--runs "$ATRIUM_MEASURE_RUNS"); fi
      echo "=== metric=$metric windows=$windows opacity=$opacity (alpha: off=1, otherwise 0.6)"
      log="$out/$metric-$windows-$opacity.log"
      if ! python3 scripts/measure.py "$metric" --windows "$windows" --opacity "$opacity" ${args[@]+"${args[@]}"} ${extra[@]+"${extra[@]}"} >"$log" 2>&1; then
        failed+=("$metric-$windows-$opacity")
        echo "    FAILED: $(grep -m1 '^FAILED' "$log" || tail -n 3 "$log")"
      else
        echo "    ok: $(grep -m1 -E '^TTI|TOTAL|^idleFrames' "$log" || true)"
      fi
      # Nothing of ours may outlive a cell.
      leftover=$(pgrep -f "Atrium UI Shell.app/Contents/MacOS/atrium-ui-shell" || true)
      if [[ -n "$leftover" ]]; then echo "    stopping leftover app processes: $leftover"; kill $leftover 2>/dev/null || true; fi
    done
  done
done
if [[ ${#failed[@]} -gt 0 ]]; then
  echo "FAILED cells (${#failed[@]}): ${failed[*]}"
  exit 1
fi
echo "all cells passed; logs in $out"
