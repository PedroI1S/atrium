#!/usr/bin/env bash
# In-app measurement: runs the release app with ATRIUM_BENCH=screencast N times and writes one
# JSON per run. Needs `pnpm tauri build --no-bundle` first. Each run uses a fresh profile dir.
#   scripts/bench.sh <runs> <out-dir> [ENV=VALUE ...]     e.g. ATRIUM_DSF=2 ATRIUM_QUALITY=80
set -euo pipefail
cd "$(dirname "$0")/.."
RUNS="${1:?runs}"; OUT="${2:?out dir}"; shift 2
BIN=src-tauri/target/release/atrium-browser-screencast
mkdir -p "$OUT"
for i in $(seq 1 "$RUNS"); do
  DATA="$(mktemp -d)"
  # Linux: sample the process tree from /proc alongside the run (D-037), into run-<i>.proc.json.
  WRAP=()
  [[ "$(uname -s)" == Linux ]] && WRAP=(python3 scripts/procsample.py "$OUT/run-$i.proc.json" --)
  env ATRIUM_BENCH=screencast ATRIUM_BENCH_OUT="$OUT/run-$i.json" ATRIUM_DATA_DIR="$DATA" "$@" ${WRAP[@]+"${WRAP[@]}"} "$BIN" >/dev/null 2>&1
  rm -rf "$DATA"
  echo "run $i -> $OUT/run-$i.json"
done
